use axum::extract::rejection::JsonRejection;
use axum::extract::{FromRequest, Request};

use crate::error::AppError;

/// `axum::Json`, but rejections come back as 400 `validation` JSON.
pub struct AppJson<T>(pub T);

impl<S, T> FromRequest<S> for AppJson<T>
where
    axum::Json<T>: FromRequest<S, Rejection = JsonRejection>,
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match axum::Json::<T>::from_request(req, state).await {
            Ok(axum::Json(value)) => Ok(AppJson(value)),
            Err(rejection) => Err(AppError::validation(rejection.body_text())),
        }
    }
}
