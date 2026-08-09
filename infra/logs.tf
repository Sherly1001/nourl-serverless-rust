# Access logs and how long anything is kept.
#
# Two sources, because they answer different questions. The Lambda's own group
# holds whatever the backend printed and one REPORT line per invocation, and it
# arrives in seconds. CloudFront holds a line per request — static files and API
# alike, since everything reaches the site through it — but is delivered in
# batches, so it is minutes behind rather than live.

# Lambda creates this group by itself on first invocation, with no expiry, and
# then it grows forever. Declaring it is the only way to put a retention on it.
# The group already exists in both workspaces, so it has to be imported once
# before the first apply — see README.
resource "aws_cloudwatch_log_group" "lambda" {
  name              = "/aws/lambda/${aws_lambda_function.api.function_name}"
  retention_in_days = var.log_retention_days
}

resource "aws_s3_bucket" "logs" {
  bucket = "${local.prefix}-logs-${data.aws_caller_identity.current.account_id}"
}

resource "aws_s3_bucket_public_access_block" "logs" {
  bucket                  = aws_s3_bucket.logs.id
  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

resource "aws_s3_bucket_lifecycle_configuration" "logs" {
  bucket = aws_s3_bucket.logs.id

  rule {
    id     = "expire"
    status = "Enabled"

    filter {}

    expiration {
      days = var.log_retention_days
    }

    # A delivery interrupted partway leaves parts behind that no lifecycle
    # expiry reaches, since they are not objects yet.
    abort_incomplete_multipart_upload {
      days_after_initiation = 7
    }
  }
}

# Vended log delivery writes as a service principal, not as us. The conditions
# are what stop another account naming this bucket as its own log destination.
data "aws_iam_policy_document" "logs" {
  statement {
    principals {
      type        = "Service"
      identifiers = ["delivery.logs.amazonaws.com"]
    }
    actions   = ["s3:PutObject"]
    resources = ["${aws_s3_bucket.logs.arn}/*"]

    condition {
      test     = "StringEquals"
      variable = "aws:SourceAccount"
      values   = [data.aws_caller_identity.current.account_id]
    }
    condition {
      test     = "ArnLike"
      variable = "aws:SourceArn"
      values   = ["arn:aws:logs:us-east-1:${data.aws_caller_identity.current.account_id}:delivery-source:*"]
    }
  }

  statement {
    principals {
      type        = "Service"
      identifiers = ["delivery.logs.amazonaws.com"]
    }
    actions   = ["s3:GetBucketAcl", "s3:ListBucket"]
    resources = [aws_s3_bucket.logs.arn]

    condition {
      test     = "StringEquals"
      variable = "aws:SourceAccount"
      values   = [data.aws_caller_identity.current.account_id]
    }
  }
}

resource "aws_s3_bucket_policy" "logs" {
  bucket = aws_s3_bucket.logs.id
  policy = data.aws_iam_policy_document.logs.json
}

# CloudFront is global, so its delivery source lives in us-east-1 whatever
# region the rest of this stack is in.
resource "aws_cloudwatch_log_delivery_source" "cloudfront" {
  provider     = aws.us_east_1
  name         = "${local.prefix}-cloudfront"
  log_type     = "ACCESS_LOGS"
  resource_arn = aws_cloudfront_distribution.main.arn
}

resource "aws_cloudwatch_log_delivery_destination" "logs_bucket" {
  provider = aws.us_east_1
  name     = "${local.prefix}-logs-bucket"
  # The same fields and order the legacy standard logs used, so anything that
  # already reads that format keeps working.
  output_format = "w3c"

  delivery_destination_configuration {
    destination_resource_arn = aws_s3_bucket.logs.arn
  }
}

resource "aws_cloudwatch_log_delivery" "cloudfront" {
  provider                 = aws.us_east_1
  delivery_source_name     = aws_cloudwatch_log_delivery_source.cloudfront.name
  delivery_destination_arn = aws_cloudwatch_log_delivery_destination.logs_bucket.arn

  s3_delivery_configuration {
    suffix_path = "/{yyyy}/{MM}/{dd}"
  }

  depends_on = [aws_s3_bucket_policy.logs]
}
