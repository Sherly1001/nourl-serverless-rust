# nourl

URL shortener in Rust: Leptos CSR frontend on S3/CloudFront, axum API on AWS
Lambda (API Gateway), MongoDB Atlas, infra managed by Terraform (AWS +
Cloudflare).

## Local dev

```sh
make mongo-up            # local MongoDB in docker (container: nourl-mongo)
cp .env.example .env     # then adjust if needed
make dev-backend         # axum server on http://localhost:9669
make test                # workspace tests (needs mongo-up)
make fmt                 # rustfmt + leptosfmt + rustywind + prettier + terraform fmt
```

The backend binary auto-detects Lambda (`AWS_LAMBDA_RUNTIME_API` env) and
otherwise runs as a plain TCP server on `PORT` (default 9669).

Dev-only note: the short link shown after creating one points at the Trunk
origin (`localhost:8080/<code>`), but Trunk only proxies `/api` to the
backend, so following it serves the SPA instead of redirecting. Test
redirects directly against the backend: `curl -i localhost:9669/<code>`. In
production both live on the same origin, so the link works.

Frontend assets are served from the root path in both dev and prod: Trunk
keeps its default `public_url`, and `frontend/dist/` maps 1:1 onto the S3
bucket root. CloudFront routes `/`, `/index.html`, `/favicon.ico`,
`/robots.txt`, `*.js`, `*.wasm` and `*.css` to S3; everything else goes to
the Lambda. Short codes can never contain a dot, so the extension patterns
cannot shadow a redirect.

## Infra

Terraform lives in `infra/`. Environments are **workspaces**, not directories:
`dev` and `prod` share one config and pick up their differences from
`infra/envs/<workspace>.tfvars`. State is in `s3://nourl-tfstate-664185729291`
(versioned, public access blocked) with S3-native locking.

```sh
make build-lambda        # cargo lambda build --release --arm64 -p backend
make tf-plan-dev         # builds the lambda, then plans the dev workspace
```

Terraform reads neither credential source on its own. AWS session credentials
live under `~/.aws/login/` behind a `login_session` key the AWS Go SDK
ignores, so it falls through to EC2 IMDS and times out even while `aws sts
get-caller-identity` works; `CLOUDFLARE_API_TOKEN` sits in `.env`, which only
the backend loads. The `TF` variable in the `Makefile` exports both. Run raw
commands the same way:

```sh
cd infra
set -a; . ../.env; set +a
eval "$(aws configure export-credentials --format env)"
terraform workspace select dev
terraform plan -var-file=envs/dev.tfvars
```

The Cloudflare token needs `Zone:DNS:Edit` on nourl.space; put it in `.env`
(gitignored) as `CLOUDFLARE_API_TOKEN`.

`MONGO_URL` is read from SSM (`/nourl-dev/mongo-url`, `/nourl/mongo-url`),
which are created out of band and land in Terraform state — accepted because
the state bucket is private.

Both environments have a real hostname — dev is `dev.nourl.space`, prod is
`nourl.space` — so both get an ACM certificate validated through Cloudflare
DNS and a proxied CNAME to CloudFront. They share one zone, so the zone id is
a default in `variables.tf` rather than a per-env tfvar. Leaving `domain_name`
empty is still supported and serves from the raw CloudFront URL.

## Layout

- `shared/` — DTOs + validation used by backend and frontend
- `backend/` — axum API (redirect at `GET /{code}`, JSON under `/api`)
- `frontend/` — Leptos CSR app (Trunk + Tailwind 4 + FlyonUI)
- `infra/` — Terraform (AWS + Cloudflare)
