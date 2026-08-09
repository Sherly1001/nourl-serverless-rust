resource "aws_cloudfront_origin_access_control" "static" {
  name                              = "${local.prefix}-oac"
  origin_access_control_origin_type = "s3"
  signing_behavior                  = "always"
  signing_protocol                  = "sigv4"
}

data "aws_cloudfront_cache_policy" "caching_optimized" {
  name = "Managed-CachingOptimized"
}

data "aws_cloudfront_cache_policy" "caching_disabled" {
  name = "Managed-CachingDisabled"
}

data "aws_cloudfront_origin_request_policy" "all_viewer_except_host" {
  name = "Managed-AllViewerExceptHostHeader"
}

locals {
  s3_origin_id  = "s3-static"
  api_origin_id = "apigw"
  api_domain    = replace(aws_apigatewayv2_api.http.api_endpoint, "https://", "")

  # Where this environment actually answers. With no domain of its own that is
  # the distribution's URL, which is a real origin OAuth can be registered
  # against — "https://" with the hostname left out is not.
  site_url = local.has_domain ? "https://${var.domain_name}" : "https://${aws_cloudfront_distribution.main.domain_name}"

  # Frontend assets live at the bucket root (Trunk uses the default public_url
  # so dev and prod URLs match). Short codes can never contain a dot
  # (`validate_code`), so the *.ext patterns cannot shadow a redirect.
  static_patterns = [
    "/index.html",
    "/favicon.ico",
    "/favicon.png",
    "/apple-touch-icon.png",
    "/robots.txt",
    "*.js",
    "*.wasm",
    "*.css",
  ]
}

resource "aws_cloudfront_distribution" "main" {
  enabled     = true
  comment     = local.prefix
  aliases     = local.has_domain ? [var.domain_name] : []
  price_class = "PriceClass_200"

  # Rewrites a request for "/" to "/index.html" before behaviours are matched,
  # so the SPA shell is served from S3 without needing a "/" path pattern (S3
  # would answer an empty key with AccessDenied).
  default_root_object = "index.html"

  origin {
    origin_id                = local.s3_origin_id
    domain_name              = aws_s3_bucket.static.bucket_regional_domain_name
    origin_access_control_id = aws_cloudfront_origin_access_control.static.id
  }

  origin {
    origin_id   = local.api_origin_id
    domain_name = local.api_domain
    custom_origin_config {
      http_port              = 80
      https_port             = 443
      origin_protocol_policy = "https-only"
      origin_ssl_protocols   = ["TLSv1.2"]
    }
  }

  # Everything unmatched is a short code or an /api call: straight to Lambda,
  # uncached, with the viewer's headers forwarded (minus Host, which has to
  # stay the API Gateway domain).
  default_cache_behavior {
    target_origin_id         = local.api_origin_id
    viewer_protocol_policy   = "redirect-to-https"
    allowed_methods          = ["GET", "HEAD", "OPTIONS", "PUT", "POST", "PATCH", "DELETE"]
    cached_methods           = ["GET", "HEAD"]
    cache_policy_id          = data.aws_cloudfront_cache_policy.caching_disabled.id
    origin_request_policy_id = data.aws_cloudfront_origin_request_policy.all_viewer_except_host.id
  }

  dynamic "ordered_cache_behavior" {
    for_each = local.static_patterns
    content {
      path_pattern           = ordered_cache_behavior.value
      target_origin_id       = local.s3_origin_id
      viewer_protocol_policy = "redirect-to-https"
      allowed_methods        = ["GET", "HEAD"]
      cached_methods         = ["GET", "HEAD"]
      cache_policy_id        = data.aws_cloudfront_cache_policy.caching_optimized.id
    }
  }

  restrictions {
    geo_restriction { restriction_type = "none" }
  }

  viewer_certificate {
    cloudfront_default_certificate = local.has_domain ? false : true
    acm_certificate_arn            = local.has_domain ? aws_acm_certificate_validation.cert[0].certificate_arn : null
    ssl_support_method             = local.has_domain ? "sni-only" : null
    minimum_protocol_version       = local.has_domain ? "TLSv1.2_2021" : "TLSv1"
  }
}
