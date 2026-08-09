data "aws_ssm_parameter" "mongo_url" {
  name = var.mongo_url_ssm_path
}

data "aws_ssm_parameter" "jwt_secret" {
  name = var.jwt_secret_ssm_path
}

data "archive_file" "lambda_zip" {
  type        = "zip"
  source_file = "${path.module}/../target/lambda/backend/bootstrap"
  output_path = "${path.module}/.build/lambda.zip"
}

resource "aws_iam_role" "lambda" {
  name = "${local.prefix}-lambda"
  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect    = "Allow"
      Principal = { Service = "lambda.amazonaws.com" }
      Action    = "sts:AssumeRole"
    }]
  })
}

resource "aws_iam_role_policy_attachment" "lambda_logs" {
  role       = aws_iam_role.lambda.name
  policy_arn = "arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole"
}

resource "aws_lambda_function" "api" {
  function_name    = "${local.prefix}-api"
  role             = aws_iam_role.lambda.arn
  runtime          = "provided.al2023"
  architectures    = ["arm64"]
  handler          = "bootstrap"
  filename         = data.archive_file.lambda_zip.output_path
  source_code_hash = data.archive_file.lambda_zip.output_base64sha256
  memory_size      = 512
  timeout          = 15

  environment {
    variables = {
      MONGO_URL             = data.aws_ssm_parameter.mongo_url.value
      MONGO_DB              = var.mongo_db
      JWT_SECRET            = data.aws_ssm_parameter.jwt_secret.value
      NOTFOUND_FALLBACK_URL = local.notfound_fallback_url
      # What the OAuth callback URL is built from, and what has to be
      # registered at each provider. Derived from wherever this environment
      # answers, so the two cannot drift apart.
      PUBLIC_BASE_URL = local.site_url
    }
  }
}
