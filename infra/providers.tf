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
}
