# Behaviour

The rules the API enforces.

## Links

- Following a code is one atomic update: it raises `hits` and stamps
  `last_hit_at`. An expired link neither redirects nor counts, without waiting
  for Mongo's TTL sweep (about once a minute).
- Anyone may create a link, signed in or not. A signed-out one is unowned.
- `expires_at` on a create or edit is RFC3339 and must be in the future.
  **Absent** leaves what is stored, an **empty string** removes the expiry, and a
  stamp sets it.
- `reset_hits: true` zeroes `hits` and clears `last_hit_at`.

### Who may write

- Unowned: anyone, signed in or not.
- Owned: the owner, or an admin the owner is reachable from. An ordinary
  account's link is reachable from every admin; an admin's link only from the
  admins above them in the chain. Never sideways, never upwards.
- Anything else answers a flat 403 with no destination in it, so the error cannot
  be used to look up other people's links.
- Every returned link carries `editable` and `claimable`, and the page draws its
  buttons from those.

### Creating over a taken code

- Unowned: overwritten silently. A signed-in creator becomes its owner.
- Owned, and you may write it: 409, carrying the existing link so the page can
  show what would change. Resend with `overwrite: true` to replace it.
- Replacing keeps the owner and the hit count. `claim` and `reset_hits` are
  separate asks.
- Renaming a link onto a taken code follows the same rules. Someone's link is
  never deleted as a side effect.

### Claiming

- Admins only: `POST /api/urls/{code}/claim`, a bulk claim, or `claim: true` on
  a write. Covers unowned links, plus owned ones the admin may write.
- Claiming an unowned link by route or bulk also clears its expiry, which is how
  an orphaned code outlives its grace period. `claim: true` on a write does not.
- Editing an orphan does not take ownership. Re-creating it signed in does, but
  the expiry stays unless the request changes it.

### Bulk

- `POST /api/urls/bulk` deletes or claims up to 100 codes in one transaction.
  Every code is checked first, and a refusal names every code it was about.

## Accounts

- Anyone may register while the password method is on. The account owns every
  link it creates.
- An admin deleting an account never deletes its links. They become unowned with
  `expires_at = min(existing, now + ORPHAN_GRACE_DAYS)` (default 7).
- Closing your own account asks what happens to your links: delete them, or
  orphan them on the same clock. An admin must resign first. A top admin cannot
  close their account.
- Logging out and changing a password revoke every session of the account.
  Changing a password then signs this device back in.

### Admin chain

- Promoting puts the account under the promoter. Promoting an existing admin
  leaves them where they are unless `promoted_by` names a new parent: yourself,
  or an admin below you, and never below their own branch.
- An admin acts only on accounts below them. A top admin is in nobody's branch,
  so the API cannot reach them.
- Nothing may be done to your own account except resigning. A top admin cannot
  resign, because nobody above could restore the flag. Remove it in the database.
- Demoting, resigning or deleting an admin asks what happens to their branch:
  demote all of it, or move it up to the nearest surviving ancestor.
- `POST /api/admin/users/bulk` promotes, demotes or deletes up to 100 accounts in
  one transaction, all or nothing. Only the explicitly selected accounts count as
  affected. The rest are reported as the branch below.
- Only a top admin may open or change sign-in settings.

## Sign-in

- Four methods: password, GitHub, Google, Facebook. A provider counts as on only
  when it is enabled **and** has both a client id and a secret.
- Secrets are stored in the Mongo `settings` document and never sent back to the
  browser. An omitted secret keeps the stored one, and an empty string removes it.
- The callback URL is built from `PUBLIC_BASE_URL`, never from `Host`.

### Which account an identity reaches

1. Identity already held: sign in to that account. In a link flow held by
   another account, fail as `oauth_taken` instead.
2. Link flow (started while signed in): attach the identity to that account.
   The flag lives in the signed state cookie, so a caller cannot choose.
3. Otherwise, a **verified** email held by exactly one account (case-insensitive)
   attaches to that account.
4. Otherwise, a new account is created with a username derived from the profile.

- Verified email: Google's `email_verified`. GitHub's primary verified address,
  or failing that the first verified one. Facebook: never.
- Disconnecting the last way into an account (no password, one identity) is
  refused. Set a password first.
