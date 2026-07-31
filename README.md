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

## Layout

- `shared/` — DTOs + validation used by backend and frontend
- `backend/` — axum API (redirect at `GET /{code}`, JSON under `/api`)
- `frontend/` — Leptos CSR app (Trunk + Tailwind 4 + FlyonUI)
- `infra/` — Terraform (AWS + Cloudflare)
