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

variable "notfound_fallback_url" {
  type    = string
  default = "https://nourl.space"
}
