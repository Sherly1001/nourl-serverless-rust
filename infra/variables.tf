variable "aws_region" {
  type    = string
  default = "ap-northeast-1"
}

variable "domain_name" {
  type        = string
  default     = ""
  description = "Custom domain. Empty = serve from the raw CloudFront URL."
}

# Both environments are hostnames inside the same nourl.space zone, so the id
# is a constant rather than a per-env tfvar.
variable "cloudflare_zone_id" {
  type    = string
  default = "22d848143cca3595d64a0fb225c639ff"
}

variable "mongo_url_ssm_path" {
  type = string
}

# Deliberately a different secret per environment: a leaked dev secret must
# not be able to mint production sessions.
variable "jwt_secret_ssm_path" {
  type = string
}

# The database inside the cluster. Both SSM connection strings point at the
# same Atlas cluster, and the backend ignores the path component of the URL
# (`client.database(db_name)`), so this is what actually keeps dev off the
# production data.
variable "mongo_db" {
  type    = string
  default = "nourl"
}

# Empty means "this environment's own site" — see local.notfound_fallback_url.
# Leaving it unset in the Lambda is not an option: the backend would fall back
# to x-forwarded-host (needs a CloudFront Function we do not deploy) and then
# to host, which API Gateway has already rewritten to its own domain.
variable "notfound_fallback_url" {
  type    = string
  default = ""
}

# Applies to both the Lambda's CloudWatch group and the CloudFront log bucket,
# so "how far back can I look" has one answer rather than two.
variable "log_retention_days" {
  type    = number
  default = 90
}
