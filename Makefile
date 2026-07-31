.PHONY: fmt fmt-check mongo-up mongo-down dev-backend dev-frontend build-frontend test

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

frontend/node_modules: frontend/package.json
	cd frontend && pnpm install
	touch $@

test: mongo-up
	cargo test --workspace

mongo-up:
	docker start nourl-mongo 2>/dev/null || docker run -d --name nourl-mongo -p 27017:27017 mongo:7

mongo-down:
	docker stop nourl-mongo
