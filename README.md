# nourl

URL shortener in Rust: Leptos CSR frontend on S3/CloudFront, axum API on AWS
Lambda (API Gateway), MongoDB Atlas, infra managed by Terraform (AWS +
Cloudflare).

- `shared/`: DTOs and validation used by backend and frontend
- `backend/`: axum API (redirect at `GET /{code}`, JSON under `/api`)
- `frontend/`: Leptos CSR app (Trunk + Tailwind 4 + FlyonUI)
- `infra/`: Terraform (AWS + Cloudflare)

The rules the API enforces (link ownership, the admin chain, sign-in) are in
[docs/behaviour.md](docs/behaviour.md).

## Local dev

```sh
make mongo-up            # local MongoDB in docker (container: nourl-mongo)
cp .env.example .env     # then adjust if needed
make dev-backend         # axum server on http://localhost:9669
make dev-frontend        # trunk serve on http://localhost:8080, proxies /api
make test                # workspace tests (needs mongo-up)
make fmt                 # rustfmt + leptosfmt + rustywind + prettier + terraform fmt
```

- `mongo-up` runs a single-node replica set with test commands on, because
  transactions and the `failCommand` tests need both. The data lives in a named
  volume, and `make mongo-reset` wipes it.
- The backend runs as a Lambda when `AWS_LAMBDA_RUNTIME_API` is set, and
  otherwise as a TCP server on `PORT` (default 9669).
- The short link the UI shows locally (`localhost:8080/<code>`) serves the SPA,
  because Trunk proxies only `/api`. Test redirects against the backend:
  `curl -i localhost:9669/<code>`.
- CloudFront sends `/`, `/index.html`, `/favicon.ico`, `/robots.txt`, `*.js`,
  `*.wasm` and `*.css` to S3, and everything else to the Lambda. Codes cannot
  contain a dot, so a static file can never shadow a redirect.

## Infra

Environments are Terraform **workspaces**, not directories. `dev` and `prod`
share one config and differ only in `infra/envs/<workspace>.tfvars`. State is in
a private, versioned S3 bucket with S3-native locking.

```sh
make build-lambda        # cargo lambda build --release --arm64 -p backend
make tf-plan-dev         # builds the lambda, then plans the dev workspace
make deploy ENV=dev      # build both, terraform apply, upload dist/, invalidate
make sync-static ENV=dev # re-upload the frontend only (no terraform)
make tf-output ENV=dev   # site_url, bucket, distribution id, api endpoint
```

`make deploy` runs `cargo lambda build`, `trunk build --release`, `terraform
apply` (interactive), `aws s3 sync` of `frontend/dist`, then a CloudFront
invalidation. `index.html` is uploaded with `no-cache`.

### Credentials

- **AWS**: `aws login`. Sessions are short, so re-run it if an apply fails
  partway. Terraform resumes from state.
- **Cloudflare**: an API token with `Zone:DNS:Edit` on nourl.space, stored in
  `.env` as `CLOUDFLARE_API_TOKEN`.

Terraform picks up neither on its own. The AWS Go SDK ignores `aws login`
sessions and times out on EC2 IMDS, and stale `AWS_*` variables in the shell
cause `ExpiredToken`. The `Makefile` handles both. To run commands by hand:

```sh
cd infra
set -a; . ../.env; set +a
eval "$(env -u AWS_ACCESS_KEY_ID -u AWS_SECRET_ACCESS_KEY -u AWS_SESSION_TOKEN \
  -u AWS_CREDENTIAL_EXPIRATION aws configure export-credentials --format env)"
terraform workspace select dev
terraform plan -var-file=envs/dev.tfvars
```

### Secrets

Created out of band in SSM, and readable from Terraform state:

| Parameter    | dev                     | prod                |
| ------------ | ----------------------- | ------------------- |
| `MONGO_URL`  | `/nourl-dev/mongo-url`  | `/nourl/mongo-url`  |
| `JWT_SECRET` | `/nourl-dev/jwt-secret` | `/nourl/jwt-secret` |

- The database comes from the `mongo_db` tfvar (`MONGO_DB`: `nourl-dev` /
  `nourl`), not from the URL, which the backend ignores for that.
- `JWT_SECRET` differs per environment, so a dev token is worthless on prod.
  Rotating it logs everybody out. Locally it comes from `.env`, and the backend
  refuses to start without it.

### Domains

- dev is `dev.nourl.space`, prod is `nourl.space`. Each gets an ACM certificate
  in us-east-1, validated through Cloudflare DNS, plus a proxied CNAME. Leave
  `domain_name` empty to serve from the raw CloudFront URL.
- The Cloudflare SSL/TLS mode must be **Full** or **Full (strict)**. Flexible
  loops forever against CloudFront's `redirect-to-https`.
- A missing static file returns 403, not 404, because S3 behind OAC will not
  confirm that a key is absent.

### Logs

Both keep `log_retention_days` (90).

- **Lambda**, `/aws/lambda/nourl-<env>-api`: a line per request (method, path,
  status, duration, client IP, user agent). Arrives within seconds. The line has
  the path only, never the query string, which can carry OAuth codes. The client
  IP comes from `cf-connecting-ip`, falling back to the first `x-forwarded-for`.

  ```sh
  aws logs tail /aws/lambda/nourl-dev-api --region ap-northeast-1 --follow
  ```

- **CloudFront access logs**: every request, static files included, in W3C
  format. They go to the logs bucket (see `infra/`), partitioned
  `/{yyyy}/{MM}/{dd}`. Delivery is batched and runs a few minutes behind.

## First admin

There is no automatic promotion. Grant the first admin by hand:

```sh
mongosh "$MONGO_URL" --eval \
  'db.getSiblingDB("nourl").users.updateOne({username:"you"},{$set:{is_admin:true}})'
```

Use `nourl-dev` for dev. That account becomes the top admin. Everything after
that happens in the app: `#/users` promotes, demotes and deletes accounts, and
`#/settings` (top admin only) turns sign-in methods on and holds the OAuth
credentials.

## OAuth

The callback is `{PUBLIC_BASE_URL}/api/auth/{provider}/callback`. Terraform sets
`PUBLIC_BASE_URL` from `domain_name`. Unset, it falls back to
`https://nourl.space`.

| Environment | `PUBLIC_BASE_URL`         | Callback to register                               |
| ----------- | ------------------------- | -------------------------------------------------- |
| local       | `http://127.0.0.1:8080`   | `http://127.0.0.1:8080/api/auth/github/callback`   |
| dev         | `https://dev.nourl.space` | `https://dev.nourl.space/api/auth/github/callback` |
| prod        | `https://nourl.space`     | `https://nourl.space/api/auth/github/callback`     |

Locally that is the Trunk origin, not port 9669. Paste each client id and secret
into `#/settings` on the matching environment.

- **GitHub** (`read:user user:email`): <https://github.com/settings/developers>
  → **OAuth Apps**, not a GitHub App. Each app holds one callback URL, so you
  need one app per environment. The secret is shown **once**.
- **Google** (`openid email profile`): **APIs & Services** → **OAuth consent
  screen** (External), then **Credentials** → **OAuth client ID** (Web
  application). One client can list all three redirect URIs. While the app is
  unpublished, only the listed **Test users** can sign in.
- **Facebook** (`email`): <https://developers.facebook.com/apps>, add **Facebook
  Login**, and set **Valid OAuth Redirect URIs**. It accepts HTTPS only, so local
  sign-in cannot work and dev is the lowest environment. In development mode only
  accounts with an app role can sign in. The App ID and App secret are under
  **App settings** → **Basic**.
