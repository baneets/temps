//! Vercel `@vercel/sandbox` SDK compatibility test suite.
//!
//! This file pins the `/v1/sandbox/*` contract to what the
//! `@vercel/sandbox` npm SDK expects, so a drive-by refactor cannot
//! silently break drop-in clients. It validates three things:
//!
//! 1. **Path coverage** — every method the SDK calls has a matching
//!    OpenAPI path declared in `SandboxApiDoc`.
//! 2. **DTO shape** — request bodies the SDK sends deserialize into our
//!    typed DTOs, and response DTOs serialize to JSON shapes the SDK
//!    can read.
//! 3. **Strict rejection** — unknown fields are refused with an
//!    `unknown field` error, matching our `deny_unknown_fields` policy.
//!
//! These tests don't need a running server. They reflect on the
//! compiled OpenAPI document and exercise the DTOs directly, which is
//! intentional: running containers per test adds flake surface without
//! catching contract drift.
//!
//! **Sync rule:** if you add a route in `sandboxes::routes()`, add the
//! path below too. CI must fail here before it fails at the SDK.

use temps_sandbox::handlers::sandboxes::{
    CreateSandboxBody, ExecBody, ExecResponse, ExtendTimeoutBody, KillJobBody, MkdirBody,
    ReadFileResponse, SandboxResponse, SourceBody, StatResponse, WriteFileBody, WriteFilesBody,
};
use temps_sandbox::handlers::SandboxApiDoc;
use utoipa::OpenApi;

/// Every Vercel SDK operation maps to a single HTTP path. Keeping this
/// list explicit rather than derived makes contract drift a test failure
/// instead of a silent "the SDK returns 404" in production.
fn expected_sdk_paths() -> Vec<&'static str> {
    vec![
        // Sandbox lifecycle (Sandbox.create, Sandbox.get, Sandbox.stop)
        "/v1/sandbox",
        "/v1/sandbox/{id}",
        "/v1/sandbox/{id}/stop",
        "/v1/sandbox/{id}/extend-timeout",
        // Exec (Command.run, Command.runDetached)
        "/v1/sandbox/{id}/exec",
        "/v1/sandbox/{id}/exec-detached",
        // Jobs (Command.status, Command.logs, Command.kill)
        "/v1/sandbox/{id}/jobs/{job_id}",
        "/v1/sandbox/{id}/jobs/{job_id}/logs",
        "/v1/sandbox/{id}/jobs/{job_id}/kill",
        // Filesystem (sandbox.readFile, writeFile, writeFiles, stat, mkdir)
        "/v1/sandbox/{id}/fs/read",
        "/v1/sandbox/{id}/fs/write",
        "/v1/sandbox/{id}/fs/write-batch",
        "/v1/sandbox/{id}/fs/stat",
        "/v1/sandbox/{id}/fs/mkdir",
        // Preview URLs (sandbox.domain(port))
        "/v1/sandbox/{id}/domain",
        // ── Temps extensions (not in the Vercel SDK) ────────────────────
        // Non-destructive lifecycle verbs. `/stop` stays SDK-compatible
        // (stop + destroy) — these add pause/resume/restart/destroy with
        // explicit semantics for users who don't want `/stop` to also
        // tear the sandbox down.
        "/v1/sandbox/{id}/pause",
        "/v1/sandbox/{id}/resume",
        "/v1/sandbox/{id}/restart",
        "/v1/sandbox/{id}/destroy",
        // Post-create source seeding (git clone / tarball extract).
        "/v1/sandbox/{id}/source",
        // List detached jobs for a sandbox. The Vercel SDK has no
        // equivalent — it gives you a Command handle at detach time and
        // expects you to track it yourself. Useful for our dashboard
        // where a human returns after the page reloaded.
        "/v1/sandbox/{id}/jobs",
    ]
}

#[test]
fn openapi_covers_every_sdk_path() {
    let api = SandboxApiDoc::openapi();
    let got = &api.paths.paths;
    for expected in expected_sdk_paths() {
        assert!(
            got.contains_key(expected),
            "OpenAPI doc missing SDK path '{}' — the `@vercel/sandbox` SDK calls this endpoint; \
             if you removed the route, also drop it from `expected_sdk_paths`.",
            expected
        );
    }
}

/// Surface-area regression guard. The SDK is stable today — any *new* path
/// surfacing here means someone added functionality not in the SDK; that
/// needs a conscious decision (do we want to advertise it in our
/// Vercel-compat doc or document it separately?), not an implicit one.
#[test]
fn openapi_does_not_advertise_unknown_paths() {
    let api = SandboxApiDoc::openapi();
    let expected: std::collections::HashSet<_> = expected_sdk_paths().into_iter().collect();
    let got = &api.paths.paths;

    // Allowlist: non-SDK paths we explicitly expose (e.g., OpenAPI self-doc)
    // aren't listed here because they're served from `handlers::mod.rs` and
    // aren't part of `SandboxApiDoc`.

    for (path, _) in got.iter() {
        assert!(
            expected.contains(path.as_str()),
            "OpenAPI doc exposes path '{}' not in the Vercel SDK surface — \
             add it to `expected_sdk_paths` with a comment, or remove it from `SandboxApiDoc::paths`.",
            path
        );
    }
}

// ── DTO shape: request bodies the SDK sends ─────────────────────────────────

/// `Sandbox.create({ image, timeoutSecs, env })` — the SDK omits optional
/// fields. Our DTO must accept a bare object and still deserialize.
#[test]
fn create_sandbox_body_accepts_empty_object() {
    let body: CreateSandboxBody =
        serde_json::from_str("{}").expect("empty object must deserialize as defaults");
    assert!(body.image.is_none());
    assert!(body.name.is_none());
    assert!(body.env.is_empty());
    assert!(body.source.is_none());
}

#[test]
fn create_sandbox_body_accepts_full_payload() {
    // Shape taken from @vercel/sandbox 1.x docs: `image`, `timeoutSecs`,
    // `env` are the documented fields.
    let json = serde_json::json!({
        "image": "node:20",
        "name": "demo",
        "timeout_secs": 600,
        "env": { "DEBUG": "1" },
    });
    let body: CreateSandboxBody =
        serde_json::from_value(json).expect("full SDK payload must deserialize");
    assert_eq!(body.image.as_deref(), Some("node:20"));
    assert_eq!(body.timeout_secs, Some(600));
    assert_eq!(body.env.get("DEBUG").map(String::as_str), Some("1"));
}

#[test]
fn create_sandbox_body_accepts_git_source() {
    let json = serde_json::json!({
        "source": { "type": "git", "url": "https://github.com/example/repo" }
    });
    let body: CreateSandboxBody = serde_json::from_value(json).expect("git source deserializes");
    assert!(matches!(body.source, Some(SourceBody::Git { .. })));
}

#[test]
fn exec_body_accepts_minimal_command() {
    // Matches `sandbox.runCommand({ cmd: ['ls', '-la'] })`.
    let body: ExecBody =
        serde_json::from_str(r#"{"cmd":["ls","-la"]}"#).expect("minimal cmd deserializes");
    assert_eq!(body.cmd, vec!["ls".to_string(), "-la".to_string()]);
    assert!(body.env.is_empty());
    assert!(body.cwd.is_none());
}

#[test]
fn write_file_body_requires_path_and_contents() {
    // Missing `contents_b64` should fail deserialization. The SDK always
    // sends content, so accepting a body without it would mask SDK bugs.
    let err = serde_json::from_str::<WriteFileBody>(r#"{"path":"/a"}"#).expect_err("must fail");
    let msg = err.to_string();
    assert!(
        msg.contains("contents_b64") || msg.contains("missing field"),
        "expected missing-field error, got: {}",
        msg
    );
}

#[test]
fn write_files_body_accepts_empty_batch() {
    // `sandbox.writeFiles([])` is legal — maps to a zero-entry batch.
    let body: WriteFilesBody =
        serde_json::from_str(r#"{"files":[]}"#).expect("empty batch deserializes");
    assert!(body.files.is_empty());
}

#[test]
fn extend_timeout_body_requires_extra_secs() {
    let err = serde_json::from_str::<ExtendTimeoutBody>("{}").expect_err("must fail");
    assert!(err.to_string().contains("extra_secs"));
}

#[test]
fn mkdir_body_requires_path() {
    let err = serde_json::from_str::<MkdirBody>("{}").expect_err("must fail");
    assert!(err.to_string().contains("path"));
}

#[test]
fn kill_job_body_defaults_force_false() {
    // `Command.kill()` (no args) must produce a valid body; our DTO
    // defaults `force` to false. The SDK also accepts `{}` for a graceful
    // SIGTERM.
    let body: KillJobBody = serde_json::from_str("{}").expect("empty body deserializes");
    assert!(!body.force);
}

// ── DTO shape: response bodies the SDK reads ────────────────────────────────

#[test]
fn sandbox_response_has_sdk_field_names() {
    // The SDK reads `id`, `status`, `created_at`, `expires_at`. Any
    // rename here would silently break it.
    let r = SandboxResponse {
        id: "sbx_abc".into(),
        name: "demo".into(),
        status: "running".into(),
        image: Some("node:20".into()),
        work_dir: "/workspace".into(),
        created_at: "2026-01-01T00:00:00Z".into(),
        expires_at: "2026-01-01T01:00:00Z".into(),
        preview_url_template: String::new(),
        preview_password_hint: None,
    };
    let v = serde_json::to_value(&r).expect("serializes");
    for field in [
        "id",
        "name",
        "status",
        "image",
        "work_dir",
        "created_at",
        "expires_at",
    ] {
        assert!(
            v.get(field).is_some(),
            "SandboxResponse is missing SDK field '{}'",
            field
        );
    }
    // Dates must end with 'Z' (UTC) — the SDK uses `new Date(iso)` which
    // is fine with or without Z, but our codebase guarantees Z.
    assert!(v["created_at"].as_str().unwrap().ends_with('Z'));
}

#[test]
fn exec_response_exposes_exit_code_and_streams() {
    let r = ExecResponse {
        exit_code: 0,
        stdout: "hi\n".into(),
        stderr: String::new(),
    };
    let v = serde_json::to_value(&r).expect("serializes");
    for field in ["exit_code", "stdout", "stderr"] {
        assert!(v.get(field).is_some(), "ExecResponse missing '{}'", field);
    }
}

#[test]
fn read_file_response_uses_base64() {
    let r = ReadFileResponse {
        path: "/a".into(),
        contents_b64: "aGk=".into(),
        size: 2,
    };
    let v = serde_json::to_value(&r).expect("serializes");
    // `contents_b64` (not `contents`) is the field name. The SDK mirrors
    // this — renaming breaks round-tripping binary data through JSON.
    assert!(v.get("contents_b64").is_some());
    assert_eq!(v["size"], 2);
}

#[test]
fn stat_response_distinguishes_missing_from_error() {
    // The SDK relies on `exists=false` rather than 404 when a file is
    // absent. Verify the field is present and both type fields coexist.
    let r = StatResponse {
        path: "/missing".into(),
        exists: false,
        is_dir: false,
        is_file: false,
        size: 0,
    };
    let v = serde_json::to_value(&r).expect("serializes");
    assert_eq!(v["exists"], false);
    assert_eq!(v["is_dir"], false);
    assert_eq!(v["is_file"], false);
}

// ── Strict rejection (deny_unknown_fields) ──────────────────────────────────

/// Spot-check that every request DTO rejects unknowns — a typo in the
/// SDK or a fork that sends extra junk will surface immediately instead
/// of being silently dropped.
#[test]
fn request_dtos_reject_unknown_fields() {
    fn reject<T: for<'de> serde::Deserialize<'de>>(label: &str, json: &str) {
        match serde_json::from_str::<T>(json) {
            Ok(_) => panic!(
                "{} accepted unknown field: {}. This weakens SDK contract guarantees.",
                label, json
            ),
            Err(e) => assert!(
                e.to_string().contains("unknown field"),
                "{}: expected 'unknown field' error, got: {}",
                label,
                e
            ),
        }
    }

    reject::<CreateSandboxBody>("CreateSandboxBody", r#"{"xxx":1}"#);
    reject::<ExecBody>("ExecBody", r#"{"cmd":["ls"],"xxx":1}"#);
    reject::<ExtendTimeoutBody>("ExtendTimeoutBody", r#"{"extra_secs":60,"xxx":1}"#);
    reject::<WriteFileBody>(
        "WriteFileBody",
        r#"{"path":"/a","contents_b64":"aA==","xxx":1}"#,
    );
    reject::<WriteFilesBody>("WriteFilesBody", r#"{"files":[],"xxx":1}"#);
    reject::<MkdirBody>("MkdirBody", r#"{"path":"/a","xxx":1}"#);
    reject::<KillJobBody>("KillJobBody", r#"{"xxx":1}"#);
}

// ── OpenAPI schema registration ─────────────────────────────────────────────

#[test]
fn openapi_registers_core_request_schemas() {
    let api = SandboxApiDoc::openapi();
    let components = api.components.expect("components present");
    for schema in [
        "CreateSandboxBody",
        "ExecBody",
        "ExtendTimeoutBody",
        "WriteFileBody",
        "WriteFilesBody",
        "MkdirBody",
        "KillJobBody",
    ] {
        assert!(
            components.schemas.contains_key(schema),
            "OpenAPI is missing request schema '{}' — external SDK generators won't see it",
            schema
        );
    }
}

#[test]
fn openapi_registers_core_response_schemas() {
    let api = SandboxApiDoc::openapi();
    let components = api.components.expect("components present");
    for schema in [
        "SandboxResponse",
        "ExecResponse",
        "ReadFileResponse",
        "StatResponse",
        "DomainResponse",
    ] {
        assert!(
            components.schemas.contains_key(schema),
            "OpenAPI is missing response schema '{}'",
            schema
        );
    }
}
