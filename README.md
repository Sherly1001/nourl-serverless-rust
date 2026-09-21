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

`mongo-up` runs mongod as a single-node replica set with test commands on: a
transaction spans more than one document and mongod only offers those on a
replica set, and `failCommand` is how a test makes a write fail halfway through
one. Production is Atlas, so a standalone container would leave those paths
untested locally.

A container whose flags no longer match replaces itself the next time the target
runs. The data lives in a named volume and survives that; `make mongo-reset`
removes the volume too, which is how to actually start over.

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

### Logs

Two of them, answering different questions.

**The Lambda's own group**, `/aws/lambda/nourl-<env>-api`, holds one line per
request — method, path, status, duration, caller IP, user agent — plus a
`REPORT` line per invocation with duration, memory and billed time. It arrives
within seconds, so it is the one to watch while something is going wrong:

```sh
aws logs tail /aws/lambda/nourl-dev-api --region ap-northeast-1 --follow
```

The request line carries the **path only, never the query string**: an OAuth
callback arrives with the authorization code and the state nonce in it, and a
log line is the last place either belongs.

The IP is the visitor's, not a proxy's. Requests arrive browser → Cloudflare →
CloudFront → API Gateway, each hop appending what it saw to `x-forwarded-for`,
so the caller is that header's first entry — but a browser can send that header
itself and Cloudflare appends to it rather than replacing it. So
`cf-connecting-ip` is preferred: Cloudflare writes it over anything the client
sent, and the distribution forwards it because the origin request policy is
`AllViewerExceptHostHeader`. It is absent only for a request that skipped
Cloudflare by hitting the CloudFront domain directly, and then the forwarded
chain is all there is.

**CloudFront access logs** hold a line per request — static files and API
alike, since everything reaches the site through CloudFront. They land in
`s3://nourl-<env>-logs-<account>/` in W3C format, partitioned `/{yyyy}/{MM}/{dd}`,
through standard logging v2 rather than the legacy `logging_config`: v2 delivers
as a service principal against a bucket policy, so the bucket keeps ACLs
disabled like the static one. Delivery is **batched, not live** — expect a few
minutes behind, occasionally longer. There is no way to make these live; that
is what real-time logs (Kinesis, billed per line) exist for.

Both keep `var.log_retention_days` (90) — CloudWatch retention on one, an S3
lifecycle rule on the other.

Lambda creates its log group by itself on first invocation, with no expiry, so
in an environment that ran before this was added the group has to be imported
once before the first apply:

```sh
terraform import -var-file=envs/dev.tfvars \
  aws_cloudwatch_log_group.lambda /aws/lambda/nourl-dev-api
```

## Links

Following a link is one atomic update: the same call that finds it raises
`hits` and stamps `last_hit_at`. An expired link is not found, so it is not
counted either — the redirect's own filter excludes it without waiting for
Mongo's TTL monitor, which sweeps about once a minute.

`expires_at` is RFC3339 and must be in the future. On a create or an edit it
carries three meanings, which is one more than `null` can: **absent** leaves
whatever is stored alone, so fixing a destination does not un-expire a link; an
**empty string** removes the expiry; a stamp sets it. The form sends the
browser's own offset — `2026-08-09T14:30:00+09:00` — rather than converting to
UTC itself, so no calendar arithmetic happens client-side.

### Replacing a link

A code you already own answers 409 rather than overwriting, and the error
carries the link it collided with, so the page can show what would change.
Sending the write again with `overwrite: true` goes through. `reset_hits` and
`claim` are asked for separately, because neither follows from replacing a
destination: a link pointing somewhere new has not necessarily stopped counting
its old visits, and fixing somebody's broken link is not a reason to acquire it.

A code **somebody else** owns is still a flat 403 with no url in it — the error
must stay useless as a way to look up other people's links. A code **nobody**
owns is overwritten silently, since an unowned link is already deletable by
anyone.

### Claiming

An orphaned code is dying of the deadline its owner's departure put on it, so
claiming an unowned link clears that expiry in the same write. Both claiming
and taking are a button on the My URLs row; the second asks first.

Editing, deleting and claiming a link somebody owns all follow the same rule:
an admin's reach runs down their own branch of the chain and no further — not
sideways into a peer's branch, and not upwards. A link owned by an ordinary
account is in nobody's branch, so any admin may act on it, and an unowned one
is anybody's. The rule is the one the chain already applies to accounts:
destroying a link upwards is refused exactly as taking it is. Every link comes
back carrying `editable` and `claimable`, because the chain that decides them
is not in anything the page holds, and the row draws its buttons from those.

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
link rather than only their own, and may edit or delete the ones owned by
accounts below them in the chain — but not delete someone else's as a side
effect of renaming their own.

Once one admin exists, the rest is done in the app: `#/users` grants and revokes
the admin flag and deletes accounts, `#/settings` turns login methods on and off
and holds the OAuth credentials.

Admins form a chain — whoever grants the flag owns that branch, and an admin can
act only on accounts below their own. The account promoted by hand above is the
root of that chain: it answers to nobody, cannot be reached through the API, and
is the only one the settings page opens for.

The one thing an admin may do to their own row is **give up the flag**. Nobody
should have to ask permission to stop being responsible for something, and
whoever promoted them can put it back. Everything else aimed at yourself stays
refused — promoting yourself resets your own place in the chain, moving yourself
leaves the branch you were put in, and deleting yourself is what would let the
last admin lock everyone out. A **root cannot resign either**: nothing sits above
them to restore it, so the flag comes off in the database, which is where it went
on.

Removing an admin, giving up your own flag, or deleting an admin's account all
ask what becomes of the admins they promoted — demote that whole branch, or hand
it to their own parent so it keeps the flag one level shallower.

An admin deleting _someone else_ never deletes their links. They become unowned
and get `expires_at = min(existing, now + 7 days)`, so an orphaned code frees
itself within a week unless somebody claims it by re-creating or editing it.

Ticking several rows on `#/users` sends **one request**, not one per row:
`POST /api/admin/users/bulk` checks every id before it writes anything and
commits the writes together, so a selection is applied whole or refused whole —
a refusal names every id it was about, not just the first. The accounts are
handled deepest-first, which is what stops a cascade reporting rows the admin
ticked as though they were collateral. A hundred ids at a time; the page splits
a longer selection, and the all-or-nothing guarantee then holds per request
rather than across the lot.

Links deliberately work the other way. Deleting `/a` has no bearing on `/b`, and
each one asks its own permission question, so a bulk delete or claim is one
request per link and a refusal on one says nothing about the rest.

**Closing your own account** is the one place that choice is yours: `#/account`
asks whether your links go with you — gone the moment the account is — or stay
up, unowned, on that same one-week clock. An admin must give up the flag before
closing their account, so the branch below them is dealt with in the open rather
than as a side effect.

## Sign-in methods

Four ways in: a password, GitHub, Google and Facebook. Each is toggled at
`#/settings`, which only the root admin can open. A provider counts as on only
once it is enabled **and** has both a client id and a secret, so a half-filled
form leaves the button off the login page rather than producing a broken
redirect.

Credentials live in the Mongo `settings` document — never in Terraform, the
repo, or an environment variable. The settings page never sends a secret back
to the browser, only whether one is stored; leaving the field blank keeps what
is already there, so saving an unrelated change cannot wipe it.

### Callback URLs

The callback is always `{PUBLIC_BASE_URL}/api/auth/{provider}/callback`. The
origin comes from configuration, never from the request's `Host` header — a
forged host would otherwise redirect the provider's code somewhere else.

| Environment | `PUBLIC_BASE_URL`         | Callback to register                               |
| ----------- | ------------------------- | -------------------------------------------------- |
| local       | `http://127.0.0.1:8080`   | `http://127.0.0.1:8080/api/auth/github/callback`   |
| dev         | `https://dev.nourl.space` | `https://dev.nourl.space/api/auth/github/callback` |
| prod        | `https://nourl.space`     | `https://nourl.space/api/auth/github/callback`     |

Locally that is the Trunk origin, not the backend's 9669: the browser talks to
Trunk, which proxies `/api` through. Unset, it falls back to
`https://nourl.space` — production, because a stray default that silently
pointed at localhost would be the wrong way round. Terraform sets it from
`domain_name`, so OAuth needs that variable filled in; the bare CloudFront URL
has no hostname to register.

### Getting the credentials

Each provider hands out a client id and a secret from its own console. The
wording in all three moves around; what follows is what to look for rather than
a guaranteed sequence of button labels. Paste both into `#/settings` on the
matching environment as soon as you have them — the secret is the part that is
hard or impossible to see again.

**GitHub** — scopes `read:user user:email`.

1. <https://github.com/settings/developers> → **OAuth Apps** → **New OAuth App**.
   Not a GitHub App: that is a different product with a different flow.
2. Homepage URL is the site's origin; **Authorization callback URL** is the one
   from the table above.
3. Save. The **Client ID** is on the page; the secret needs **Generate a new
   client secret** and is shown **once**. Lose it and you generate another.
4. A GitHub OAuth App holds exactly **one** callback URL, so local, dev and prod
   each need their own app — three apps, three pairs of credentials.

The email comes from `/user/emails` and counts as verified only when it is the
primary address and GitHub says it is verified.

**Google** — scopes `openid email profile`.

1. <https://console.cloud.google.com> → pick or create a project.
2. **APIs & Services** → **OAuth consent screen**: user type **External**, then
   an app name and a support email. Nothing here needs an API to be enabled —
   the scopes are the non-sensitive ones, so no verification review.
3. While the consent screen is unpublished only accounts listed under **Test
   users** can sign in, and the sign-in page says so. Add your own address.
4. **Credentials** → **Create credentials** → **OAuth client ID** → application
   type **Web application**. Under **Authorized redirect URIs** add all three
   rows from the table — Google accepts a list, so one client covers every
   environment.
5. The client id and secret appear on save and stay readable from the client's
   own page afterwards.

**Facebook** — scope `email`.

1. <https://developers.facebook.com/apps> → **Create app**, use case
   **Authenticate and request data from users with Facebook Login**.
2. Add the **Facebook Login** product, then its **Settings** → **Valid OAuth
   Redirect URIs** → the URL from the table. HTTPS only: `127.0.0.1` is
   refused, so a local flow cannot be tested against Facebook at all. Dev is the
   lowest environment it works on.
3. **App settings** → **Basic**: the **App ID** is the client id, and **App
   secret** → **Show** is the secret. Both stay readable.
4. A new app is in development mode and admits only accounts with a role on it
   — admins, developers, testers, added under **App roles**. Going live to
   everybody else needs App Review and business verification for `email`.

Facebook's email is **always** treated as unverified here, whatever the response
claims, so it never links to an existing account.

### What an identity attaches to

An account is found by the provider's own user id first. Failing that, a
**verified** email matches an existing account and links to it. An unverified
one never does — it would let anyone who can type your address into a provider
walk into your account — so it creates a new account instead. Facebook
therefore never links by email at all.

Signing in while already logged in attaches the identity to the account you are
holding rather than switching accounts. Which of the two it is riding on the
signed state cookie, not on a query parameter, so a caller cannot pick.

Disconnecting is refused when it is the last way in: an account with no password
and one provider would still exist with nobody able to reach it. Set a password
first, then disconnect.

## Layout

- `shared/` — DTOs + validation used by backend and frontend
- `backend/` — axum API (redirect at `GET /{code}`, JSON under `/api`)
- `frontend/` — Leptos CSR app (Trunk + Tailwind 4 + FlyonUI)
- `infra/` — Terraform (AWS + Cloudflare)
