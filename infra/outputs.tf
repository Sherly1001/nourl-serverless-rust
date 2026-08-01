output "api_endpoint" {
  value = aws_apigatewayv2_api.http.api_endpoint
}

output "static_bucket" {
  value = aws_s3_bucket.static.bucket
}

output "distribution_id" {
  value = aws_cloudfront_distribution.main.id
}

output "cloudfront_domain" {
  value = aws_cloudfront_distribution.main.domain_name
}

output "site_url" {
  value = local.has_domain ? "https://${var.domain_name}" : "https://${aws_cloudfront_distribution.main.domain_name}"
}
