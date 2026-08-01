terraform {
  required_version = ">= 1.10"
  required_providers {
    aws        = { source = "hashicorp/aws", version = "~> 5.0" }
    cloudflare = { source = "cloudflare/cloudflare", version = "~> 5.0" }
    archive    = { source = "hashicorp/archive", version = "~> 2.0" }
  }
  backend "s3" {
    bucket       = "nourl-tfstate-664185729291"
    key          = "nourl/terraform.tfstate"
    region       = "ap-northeast-1"
    use_lockfile = true
  }
}
