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
make deploy ENV=dev      # build both, terraform apply, upload dist/, invalidate
make sync-static ENV=dev # re-upload the frontend only (no terraform)
make tf-output ENV=dev   # site_url, bucket, distribution id, api endpoint
```

`make deploy` is the whole pipeline: `cargo lambda build` → `trunk build
--release` → `terraform apply` (interactive, shows the plan) → `aws s3 sync` of
`frontend/dist` → CloudFront invalidation. `index.html` is uploaded separately
with `no-cache` because its asset hashes change on every build.

### Credentials

Two things need setting up once:

- **AWS** — `aws login`. Sessions are short-lived, so re-run it if an apply
  fails partway; terraform resumes from state with no cleanup needed.
- **Cloudflare** — an API token with `Zone:DNS:Edit` on nourl.space, in `.env`
  (gitignored) as `CLOUDFLARE_API_TOKEN`.

Neither reaches Terraform by itself. `aws login` keeps AWS session credentials
under `~/.aws/login/` behind a `login_session` key the AWS Go SDK ignores, so
it falls through to EC2 IMDS and times out even while `aws sts
get-caller-identity` works; `.env` is only loaded by the backend. The
`Makefile` exports both before every terraform and `aws` call, and strips any
`AWS_*` inherited from the shell first — stale ones outrank every other source
and produce a confusing `ExpiredToken` that surviving `aws login` does not fix.
If you run commands by hand, do the same:

```sh
cd infra
set -a; . ../.env; set +a
eval "$(env -u AWS_ACCESS_KEY_ID -u AWS_SECRET_ACCESS_KEY -u AWS_SESSION_TOKEN \
  -u AWS_CREDENTIAL_EXPIRATION aws configure export-credentials --format env)"
terraform workspace select dev
terraform plan -var-file=envs/dev.tfvars
```

`MONGO_URL` is read from SSM (`/nourl-dev/mongo-url`, `/nourl/mongo-url`),
which are created out of band and land in Terraform state — accepted because
the state bucket is private.

Sessions are signed with `JWT_SECRET`, read the same way from
`/nourl-dev/jwt-secret` and `/nourl/jwt-secret` — both SecureString, and
deliberately **different** per environment so a dev token is worthless against
prod. Locally it comes from `.env`; the backend refuses to start without it.
Rotating a secret invalidates every session signed with the old one, which is
the intended way to log everybody out.

Those two mongo parameters are currently **identical**: same Atlas cluster,
same path. What separates the environments is the `mongo_db` tfvar, which
becomes `MONGO_DB` — `nourl-dev` for dev, `nourl` for prod. The backend selects the
database with `client.database(db_name)` and ignores the path in the
connection string, so changing the URL alone would not isolate anything.

Both environments have a real hostname — dev is `dev.nourl.space`, prod is
`nourl.space` — so both get an ACM certificate (issued in us-east-1, the only
region CloudFront accepts) validated through Cloudflare DNS, plus a proxied
CNAME to the distribution. They share one zone, so the zone id is a default in
`variables.tf` rather than a per-env tfvar. Leaving `domain_name` empty is
still supported and serves from the raw CloudFront URL.

The zone's SSL/TLS mode must be **Full** or **Full (strict)**. Flexible would
loop forever against CloudFront's `redirect-to-https`.

CloudFront serves `/` from S3 via `default_root_object`, since S3 answers an
empty key with AccessDenied rather than the index. Named static files are
matched by the `ordered_cache_behavior` patterns; everything else falls
through to the Lambda, which is what makes `GET /{code}` redirects work. A
request for a static file that does not exist returns 403, not 404 — that is
S3 through OAC declining to confirm the key is missing.

## Accounts

Anyone can register; the account owns every link it creates. Links with no
owner — everything from before this phase — stay anonymously editable, and
creating over one claims it.

There is no automatic promotion, so the **first admin is granted by hand**:

```sh
mongosh "$MONGO_URL" --eval \
  'db.getSiblingDB("nourl").users.updateOne({username:"you"},{$set:{is_admin:true}})'
```

Use `nourl-dev` instead of `nourl` for the dev database. An admin sees every
link rather than only their own, and may edit any of them — but not delete
someone else's as a side effect of renaming their own.

## Layout

- `shared/` — DTOs + validation used by backend and frontend
- `backend/` — axum API (redirect at `GET /{code}`, JSON under `/api`)
- `frontend/` — Leptos CSR app (Trunk + Tailwind 4 + FlyonUI)
- `infra/` — Terraform (AWS + Cloudflare)
