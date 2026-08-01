provider "aws" {
  region = var.aws_region
}

# ACM certificates for CloudFront must live in us-east-1 (Task 11).
provider "aws" {
  alias  = "us_east_1"
  region = "us-east-1"
}

# Auth via CLOUDFLARE_API_TOKEN env var
provider "cloudflare" {}

data "aws_caller_identity" "current" {}

locals {
  prefix     = "nourl-${terraform.workspace}"
  has_domain = var.domain_name != ""

  # Unknown codes land back on the environment's own front page, so dev never
  # bounces visitors into prod. A domainless environment has no front page of
  # its own worth naming, so it falls back to the production site.
  notfound_fallback_url = coalesce(
    var.notfound_fallback_url,
    local.has_domain ? "https://${var.domain_name}" : "https://nourl.space",
  )
}
