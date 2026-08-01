use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use shared::{ApiError, ApiErrorBody};

#[derive(Debug)]
pub struct AppError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
    /// Form field this error belongs to, when there is exactly one. Serialised
    /// so a client can render the message under that input.
    pub field: Option<&'static str>,
}

impl AppError {
    /// Attaches the field this error is about. Never call it on an error whose
    /// whole point is ambiguity, such as a failed login.
    pub fn on_field(mut self, field: &'static str) -> Self {
        self.field = Some(field);
        self
    }

    pub fn validation(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: "validation",
            message: message.into(),
            field: None,
        }
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            code: "forbidden",
            message: message.into(),
            field: None,
        }
    }

    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            code: "unauthenticated",
            message: message.into(),
            field: None,
        }
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code: "conflict",
            message: message.into(),
            field: None,
        }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: "not_found",
            message: message.into(),
            field: None,
        }
    }

    pub fn not_implemented() -> Self {
        Self {
            status: StatusCode::NOT_IMPLEMENTED,
            code: "not_implemented",
            message: "available in a later phase".into(),
            field: None,
        }
    }

    pub fn internal(err: impl std::fmt::Display) -> Self {
        tracing::error!(error = %err, "internal error");
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal",
            message: "internal server error".into(),
            field: None,
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let body = ApiError {
            error: ApiErrorBody {
                code: self.code.into(),
                message: self.message,
                field: self.field.map(String::from),
            },
        };
        (self.status, Json(body)).into_response()
    }
}

impl From<mongodb::error::Error> for AppError {
    fn from(err: mongodb::error::Error) -> Self {
        AppError::internal(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;

    async fn body_of(resp: Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn every_constructor_maps_to_spec_shape() {
        let cases: Vec<(AppError, StatusCode, &str)> = vec![
            (
                AppError::validation("bad code"),
                StatusCode::BAD_REQUEST,
                "validation",
            ),
            (
                AppError::forbidden("not yours"),
                StatusCode::FORBIDDEN,
                "forbidden",
            ),
            (
                AppError::not_found("missing"),
                StatusCode::NOT_FOUND,
                "not_found",
            ),
            (
                AppError::unauthorized("no session"),
                StatusCode::UNAUTHORIZED,
                "unauthenticated",
            ),
            (
                AppError::conflict("code taken"),
                StatusCode::CONFLICT,
                "conflict",
            ),
            (
                AppError::not_implemented(),
                StatusCode::NOT_IMPLEMENTED,
                "not_implemented",
            ),
            (
                AppError::internal("db exploded"),
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
            ),
        ];
        for (err, status, code) in cases {
            let resp = err.into_response();
            assert_eq!(resp.status(), status, "{code}");
            let v = body_of(resp).await;
            assert_eq!(v["error"]["code"], code);
            assert!(v["error"]["message"].is_string());
        }
    }

    #[tokio::test]
    async fn internal_error_hides_details() {
        let resp = AppError::internal("mongo password leaked here").into_response();
        let v = body_of(resp).await;
        assert_eq!(v["error"]["message"], "internal server error");
    }
}
