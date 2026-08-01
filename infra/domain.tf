# CloudFront only accepts certificates issued in us-east-1, whatever region the
# rest of the stack lives in.
resource "aws_acm_certificate" "cert" {
  count             = local.has_domain ? 1 : 0
  provider          = aws.us_east_1
  domain_name       = var.domain_name
  validation_method = "DNS"

  lifecycle {
    create_before_destroy = true
  }
}

resource "cloudflare_dns_record" "cert_validation" {
  for_each = local.has_domain ? {
    for dvo in aws_acm_certificate.cert[0].domain_validation_options :
    dvo.domain_name => dvo
  } : {}

  zone_id = var.cloudflare_zone_id
  # ACM hands these back fully qualified with a trailing dot; Cloudflare stores
  # them without one, so keeping the dot means a diff on every single plan.
  name    = trimsuffix(each.value.resource_record_name, ".")
  type    = each.value.resource_record_type
  content = trimsuffix(each.value.resource_record_value, ".")
  ttl     = 60
  proxied = false
}

resource "aws_acm_certificate_validation" "cert" {
  count           = local.has_domain ? 1 : 0
  provider        = aws.us_east_1
  certificate_arn = aws_acm_certificate.cert[0].arn
  validation_record_fqdns = [
    for r in cloudflare_dns_record.cert_validation : trimsuffix(r.name, ".")
  ]
}

# Proxied (orange cloud). Cloudflare's SSL mode for this zone must be Full or
# Full (strict) — Flexible would loop against `redirect-to-https`.
resource "cloudflare_dns_record" "site" {
  count   = local.has_domain ? 1 : 0
  zone_id = var.cloudflare_zone_id
  name    = var.domain_name
  type    = "CNAME"
  content = aws_cloudfront_distribution.main.domain_name
  ttl     = 1
  proxied = true
}
