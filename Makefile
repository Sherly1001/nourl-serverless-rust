.PHONY: fmt fmt-check mongo-up mongo-down mongo-clean mongo-reset dev-backend dev-frontend build-frontend build-lambda test tf-plan-dev deploy sync-static tf-output

ENV ?= dev

# Terraform reads neither of the two credential sources it needs. `aws login`
# keeps AWS session credentials under ~/.aws/login/ behind a `login_session`
# key its Go SDK ignores, and CLOUDFLARE_API_TOKEN lives in .env, which only
# the backend loads. Both get exported here.
#
# `env -u` drops AWS_* inherited from the caller's shell first: a stale set
# from an earlier session outranks the CLI's own credential lookup, so the
# export fails and terraform then inherits those same dead keys and reports
# ExpiredToken. Capturing before eval makes that failure fatal instead of
# silent.
TF_ENV = set -a; [ -f $(CURDIR)/.env ] && . $(CURDIR)/.env; set +a; \
	 creds=$$(env -u AWS_ACCESS_KEY_ID -u AWS_SECRET_ACCESS_KEY -u AWS_SESSION_TOKEN -u AWS_CREDENTIAL_EXPIRATION \
	   aws configure export-credentials --format env) || exit 1; \
	 eval "$$creds";
TF = $(TF_ENV) terraform

# Direct CLI calls need the same shielding — they resolve credentials
# themselves and would pick the caller's stale AWS_* right back up.
AWS = env -u AWS_ACCESS_KEY_ID -u AWS_SECRET_ACCESS_KEY -u AWS_SESSION_TOKEN -u AWS_CREDENTIAL_EXPIRATION aws

fmt:
	cargo fmt --all
	leptosfmt frontend/src
	rustywind --write frontend/src
	prettier --write "**/*.{html,css,json,yaml,md}" --log-level warn
	terraform fmt -recursive infra 2>/dev/null || true

fmt-check:
	cargo fmt --all -- --check
	leptosfmt --check frontend/src
	rustywind --check-formatted frontend/src
	prettier --check "**/*.{html,css,json,yaml,md}" --log-level warn
	terraform fmt -check -recursive infra 2>/dev/null || true

# Rebuilds and restarts on save, to match `trunk serve` on the frontend side.
# Watching only the two crates the binary is built from keeps a frontend edit
# from bouncing the API — `trunk serve` proxies to it and would drop the
# connection.
dev-backend: mongo-up
	@command -v cargo-watch >/dev/null \
	  || { echo "cargo-watch not installed: cargo install cargo-watch"; exit 1; }
	cargo watch -c -w backend/src -w shared/src -x 'run -p backend'

dev-frontend: frontend/node_modules
	cd frontend && trunk serve --open

build-frontend: frontend/node_modules
	cd frontend && trunk build --release

build-lambda:
	cargo lambda build --release --arm64 -p backend

tf-plan-dev: build-lambda
	cd infra && $(TF) workspace select dev && terraform plan -var-file=envs/dev.tfvars

# ENV picks the terraform workspace and its tfvars file: make deploy ENV=prod
deploy: build-lambda build-frontend
	cd infra && $(TF) workspace select $(ENV) && terraform apply -var-file=envs/$(ENV).tfvars
	$(MAKE) sync-static ENV=$(ENV)

sync-static:
	$(eval BUCKET := $(shell cd infra && $(TF) workspace select $(ENV) >/dev/null && terraform output -raw static_bucket))
	$(eval DIST := $(shell cd infra && $(TF) workspace select $(ENV) >/dev/null && terraform output -raw distribution_id))
	# dist/ maps 1:1 onto the bucket root, so served URLs match `trunk serve`.
	# index.html is uploaded separately with no-cache since its asset hashes change.
	$(AWS) s3 sync frontend/dist "s3://$(BUCKET)/" --exclude index.html --delete
	$(AWS) s3 cp frontend/dist/index.html "s3://$(BUCKET)/index.html" --cache-control no-cache
	$(AWS) cloudfront create-invalidation --distribution-id $(DIST) --paths "/*" --no-cli-pager

tf-output:
	cd infra && $(TF) workspace select $(ENV) >/dev/null && terraform output

frontend/node_modules: frontend/package.json
	cd frontend && pnpm install
	touch $@

test: mongo-up mongo-clean
	cargo test --workspace

# `--ulimit nofile`: docker's default of 1024 is far below what mongod wants.
# Each throwaway test database costs WiredTiger a handful of file handles, so a
# parallel run against a few dozen of them exhausts the limit, and mongod
# answers that with a fatal assertion rather than an error — the container dies
# mid-run and every remaining test fails on "connection refused".
# `--replSet`: a transaction spans more than one document, which mongod only
# offers on a replica set. Production is Atlas, so a standalone container is the
# odd one out — and the code path that matters there would be the one nothing
# local could exercise. One member is enough to elect itself.
mongo-up:
	@# A container missing any of the flags would start happily and then fail
	@# the tests that need them, so it is replaced rather than reused. The data
	@# is in a named volume, which is what makes replacing it cheap: it survives
	@# the container and is picked up again by the next one.
	@if docker inspect nourl-mongo >/dev/null 2>&1 \
	  && ! docker inspect -f '{{json .Args}}' nourl-mongo \
	    | grep -q 'replSet.*enableTestCommands'; then \
	  echo "recreating nourl-mongo with a replica set and test commands"; \
	  docker rm -f nourl-mongo >/dev/null; \
	fi
	@# `enableTestCommands`: `failCommand` is how a test makes a write fail
	@# halfway through a transaction, which is the only way to prove the rest of
	@# it rolls back. Off by default, and rightly so — but this container is
	@# reachable from nowhere but here.
	docker start nourl-mongo 2>/dev/null || docker run -d --name nourl-mongo \
	  -p 27017:27017 --ulimit nofile=64000:64000 -v nourl-mongo-data:/data/db mongo:7 \
	  --replSet rs0 --setParameter enableTestCommands=1
	@# `docker start` returns as soon as the container exists, not when mongod is
	@# listening, so anything that connects straight after it races the startup
	@# and fails with ECONNREFUSED.
	@for i in $$(seq 30); do \
	  docker exec nourl-mongo mongosh --quiet --eval 'db.adminCommand({ping:1})' >/dev/null 2>&1 && break; \
	  sleep 1; \
	done
	@# The member is addressed as 127.0.0.1 because that is the only name a
	@# driver outside the container can reach it by; the default would be the
	@# container's own hostname, which resolves nowhere on the host.
	@docker exec nourl-mongo mongosh --quiet --eval \
	  'try { rs.status() } catch (e) { rs.initiate({_id: "rs0", members: [{_id: 0, host: "127.0.0.1:27017"}]}) }' >/dev/null
	@# Answering a ping is not the same as being writable: a fresh member spends
	@# a moment in STARTUP2 before it elects itself, and a write in that window
	@# fails with NotWritablePrimary.
	@for i in $$(seq 30); do \
	  docker exec nourl-mongo mongosh --quiet --eval 'db.hello().isWritablePrimary' 2>/dev/null | grep -q true && exit 0; \
	  sleep 1; \
	done; \
	echo "mongod did not become writable in 30s" >&2; exit 1

# Every test builds a throwaway `nourl_test_<uuid>` database and cannot drop it
# on the way out — Drop cannot await, and a panicking test would skip an
# explicit teardown anyway. Sweeping before each run keeps them from piling up
# without ever deleting a database the current run is using.
mongo-clean: mongo-up
	docker exec nourl-mongo mongosh --quiet --eval \
	  'db.getMongo().getDBNames().filter(n => n.startsWith("nourl_test_")).forEach(n => db.getSiblingDB(n).dropDatabase())'

mongo-down:
	docker stop nourl-mongo

# The volume outlives `docker rm`, which is the point of it — this is the way
# to actually start over.
mongo-reset:
	-docker rm -f nourl-mongo
	docker volume rm nourl-mongo-data
