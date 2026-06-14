use crate::state::AppState;
use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Router,
};
use tokio_util::io::ReaderStream;

/// Builds the axum router for serving landing pages and downloads.
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/d/:token", get(landing_page))
        .route("/d/:token/download", post(download))
        .with_state(state)
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    format!("{size:.1} {}", UNITS[unit])
}

async fn landing_page(Path(token): Path<String>, State(state): State<AppState>) -> Response {
    let share = { state.registry.lock().unwrap().get(&token).cloned() };
    let Some(share) = share else {
        return (StatusCode::NOT_FOUND, "Link not found or revoked.").into_response();
    };
    let needs_pw = share.password_hash.is_some();
    let pw_field = if needs_pw {
        r#"<input type="password" name="password" placeholder="Password" required />"#
    } else {
        ""
    };
    let html = format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><title>{name}</title>
<style>body{{font-family:system-ui;max-width:32rem;margin:4rem auto;text-align:center}}
button{{padding:.6rem 1.2rem;font-size:1rem;cursor:pointer}}
input{{padding:.5rem;margin:.5rem;font-size:1rem}}</style></head>
<body><h1>{name}</h1><p>{size}</p>
<form method="post" action="/d/{token}/download">{pw_field}<br/>
<button type="submit">Download</button></form></body></html>"#,
        name = share.name,
        size = human_size(share.size),
        token = token,
        pw_field = pw_field,
    );
    Html(html).into_response()
}

#[derive(serde::Deserialize, Default)]
struct DownloadForm {
    password: Option<String>,
}

async fn download(
    Path(token): Path<String>,
    State(state): State<AppState>,
    form: Option<axum::Form<DownloadForm>>,
) -> Response {
    let share = { state.registry.lock().unwrap().get(&token).cloned() };
    let Some(share) = share else {
        return (StatusCode::NOT_FOUND, "Link not found or revoked.").into_response();
    };

    if let Some(hash) = &share.password_hash {
        let provided = form.and_then(|f| f.0.password).unwrap_or_default();
        if !crate::password::verify_password(&provided, hash) {
            return (StatusCode::UNAUTHORIZED, "Incorrect password.").into_response();
        }
    }

    let file = match tokio::fs::File::open(&share.file_path).await {
        Ok(f) => f,
        Err(_) => return (StatusCode::GONE, "File no longer available.").into_response(),
    };

    {
        let mut reg = state.registry.lock().unwrap();
        if let Some(s) = reg.get_mut(&token) {
            s.download_count += 1;
        }
    }

    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);
    (
        [
            (header::CONTENT_TYPE, "application/octet-stream".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{}\"", share.name),
            ),
        ],
        body,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::share::Share;
    use axum::body::to_bytes;
    use axum::http::Request;
    use std::io::Write;
    use std::path::PathBuf;
    use tower::ServiceExt; // for `oneshot`

    fn state_with_share(share: Share) -> AppState {
        let st = AppState::new(0, "cloudflared".into());
        st.registry.lock().unwrap().insert(share);
        st
    }

    fn temp_file(contents: &[u8]) -> (tempfile::NamedTempFile, PathBuf) {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(contents).unwrap();
        let path = f.path().to_path_buf();
        (f, path)
    }

    #[tokio::test]
    async fn landing_page_shows_name() {
        let (_keep, path) = temp_file(b"hello");
        let share = Share::new("tok1".into(), path, "report.pdf".into(), 5, None);
        let app = build_router(state_with_share(share));
        let res = app
            .oneshot(Request::builder().uri("/d/tok1").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        assert!(String::from_utf8_lossy(&body).contains("report.pdf"));
    }

    #[tokio::test]
    async fn unknown_token_is_404() {
        let app = build_router(AppState::new(0, "cloudflared".into()));
        let res = app
            .oneshot(Request::builder().uri("/d/nope").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn download_streams_file_and_increments_count() {
        let (_keep, path) = temp_file(b"hello world");
        let share = Share::new("tok2".into(), path, "a.txt".into(), 11, None);
        let st = state_with_share(share);
        let app = build_router(st.clone());
        let res = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/d/tok2/download")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body[..], b"hello world");
        assert_eq!(st.registry.lock().unwrap().get("tok2").unwrap().download_count, 1);
    }
}
