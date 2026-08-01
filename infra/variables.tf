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

# The database inside the cluster. Both SSM connection strings point at the
# same Atlas cluster, and the backend ignores the path component of the URL
# (`client.database(db_name)`), so this is what actually keeps dev off the
# production data.
variable "mongo_db" {
  type    = string
  default = "nourl"
}

variable "notfound_fallback_url" {
  type    = string
  default = "https://nourl.space"
}
