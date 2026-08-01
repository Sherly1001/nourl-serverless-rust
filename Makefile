.PHONY: fmt fmt-check mongo-up mongo-down dev-backend dev-frontend build-frontend build-lambda test tf-plan-dev deploy sync-static tf-output

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

dev-backend: mongo-up
	cargo run -p backend

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

test: mongo-up
	cargo test --workspace

mongo-up:
	docker start nourl-mongo 2>/dev/null || docker run -d --name nourl-mongo -p 27017:27017 mongo:7

mongo-down:
	docker stop nourl-mongo
