use crate::error::AppError;
use axum::http::HeaderMap;

pub fn key<'a>(headers: &'a HeaderMap, body: Option<&'a str>) -> Result<Option<&'a str>, AppError> {
    let header = headers
        .get("idempotency-key")
        .map(|v| {
            v.to_str()
                .map_err(|_| AppError::BadRequest("invalid idempotency key".into()))
        })
        .transpose()?;
    let value = body.or(header).filter(|v| !v.is_empty());
    if value.is_some_and(|v| v.len() > 128 || v.chars().any(char::is_control)) {
        return Err(AppError::BadRequest(
            "idempotency key must be at most 128 characters".into(),
        ));
    }
    Ok(value)
}
