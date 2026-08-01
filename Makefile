.PHONY: fmt fmt-check mongo-up mongo-down dev-backend dev-frontend build-frontend build-lambda test tf-plan-dev

# Terraform reads neither of the two credential sources it needs. `aws login`
# keeps AWS session credentials under ~/.aws/login/ behind a `login_session`
# key its Go SDK ignores, and CLOUDFLARE_API_TOKEN lives in .env, which only
# the backend loads. Export both before every terraform call.
TF = set -a; [ -f $(CURDIR)/.env ] && . $(CURDIR)/.env; set +a; \
     eval "$$(aws configure export-credentials --format env)"; terraform

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
	cd infra && $(TF) workspace select dev && $(TF) plan -var-file=envs/dev.tfvars

frontend/node_modules: frontend/package.json
	cd frontend && pnpm install
	touch $@

test: mongo-up
	cargo test --workspace

mongo-up:
	docker start nourl-mongo 2>/dev/null || docker run -d --name nourl-mongo -p 27017:27017 mongo:7

mongo-down:
	docker stop nourl-mongo
