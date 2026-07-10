//! vpnrouter-gateway: schema | check | plan | apply | rollback | status |
//! doctor | explain | detect-interfaces.
//! Only apply/rollback mutate host state, and only with --yes.
//! Output contract: exactly one JSON envelope on stdout, exit codes in error.rs.

mod apply;
mod config;
mod error;
mod plan;
mod redact;
mod render;
mod status;
mod subscription;
#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use error::{ok_envelope, CliError, Detail, Suggestion};
use lexopt::prelude::*;
use serde_json::json;
use subscription::Fetcher as _;

const SCHEMA: &str = include_str!("../schema/gateway.schema.json");
const USAGE: &str = "usage: vpnrouter-gateway <schema|check|plan|apply|rollback|status|doctor|explain|render|resolve-subscription|detect-interfaces> [--config PATH] [--state-dir DIR] [--out DIR] [--yes] [--allow-ssh-risk] [--source IP] [--dest IP] [--proto tcp|udp] [--port N] [--url URL] [--active NAME] [--file PATH] [--json]";

fn main() {
    let (out, code) = match run() {
        Ok(s) => (s, 0),
        Err(e) => (e.to_json(), e.exit),
    };
    println!("{out}");
    std::process::exit(code);
}

fn run() -> Result<String, CliError> {
    let mut parser = lexopt::Parser::from_env();
    let cmd = match parser.next().map_err(|e| usage(&e.to_string()))? {
        Some(Value(v)) => v
            .into_string()
            .map_err(|_| usage("command is not valid UTF-8"))?,
        _ => return Err(usage(USAGE)),
    };

    let mut config_path: Option<PathBuf> = None;
    let mut state_dir = PathBuf::from(plan::DEFAULT_STATE_DIR);
    let mut yes = false;
    let mut allow_ssh_risk = false;
    let mut source: Option<String> = None;
    let mut dest: Option<String> = None;
    let mut proto: Option<String> = None;
    let mut port: Option<String> = None;
    let mut url: Option<String> = None;
    let mut active: Option<String> = None;
    let mut file: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    while let Some(arg) = parser.next().map_err(|e| usage(&e.to_string()))? {
        let val = |parser: &mut lexopt::Parser| -> Result<String, CliError> {
            parser
                .value()
                .map_err(|e| usage(&e.to_string()))?
                .into_string()
                .map_err(|_| usage("argument value is not valid UTF-8"))
        };
        match arg {
            Long("config") => config_path = Some(PathBuf::from(val(&mut parser)?)),
            Long("state-dir") => state_dir = PathBuf::from(val(&mut parser)?),
            Long("out") => out = Some(PathBuf::from(val(&mut parser)?)),
            Long("yes") => yes = true,
            Long("allow-ssh-risk") => allow_ssh_risk = true,
            Long("source") => source = Some(val(&mut parser)?),
            Long("dest") => dest = Some(val(&mut parser)?),
            Long("proto") => proto = Some(val(&mut parser)?),
            Long("port") => port = Some(val(&mut parser)?),
            Long("url") => url = Some(val(&mut parser)?),
            Long("active") => active = Some(val(&mut parser)?),
            Long("file") => file = Some(PathBuf::from(val(&mut parser)?)),
            // JSON is the only output format for now; accepted for forward compatibility.
            Long("json") => {}
            other => return Err(usage(&format!("unexpected argument {other:?}; {USAGE}"))),
        }
    }

    match cmd.as_str() {
        "schema" => cmd_schema(),
        "check" => cmd_check(&need_config(config_path)?),
        "plan" => cmd_plan(&need_config(config_path)?, &state_dir),
        "apply" => cmd_apply(&need_config(config_path)?, &state_dir, yes, allow_ssh_risk),
        "rollback" => apply::rollback(&state_dir, yes, &mut apply::RealNft),
        "status" => cmd_status(config_path, &state_dir),
        "doctor" => cmd_doctor(&need_config(config_path)?, &state_dir),
        "explain" => cmd_explain(&need_config(config_path)?, source, dest, proto, port),
        "render" => cmd_render(&need_config(config_path)?, &state_dir, out),
        "resolve-subscription" => {
            cmd_resolve(config_path, &state_dir, url, file.as_deref(), active)
        }
        "detect-interfaces" => Ok(ok_envelope(
            json!({ "interfaces": status::detect_interfaces()? }),
        )),
        other => Err(usage(&format!("unknown command \"{other}\"; {USAGE}"))),
    }
}

/// Proxy mode is authoring-only: apply/rollback/doctor/explain manage or read a
/// host gateway. Refuse loudly and point at `render --out`.
fn refuse_if_proxy(cfg: &config::GatewayConfig) -> Result<(), CliError> {
    if cfg.mode == config::Mode::Proxy {
        return Err(CliError {
            exit: 2,
            code: "PROXY_MODE_NOT_APPLYABLE",
            message: "this command manages a host gateway; proxy mode is authoring-only".to_string(),
            details: Vec::new(),
            suggestions: vec![Suggestion {
                command: "vpnrouter-gateway render --config /etc/vpnrouter/gateway.toml --out ./out --json".to_string(),
                reason: "Render the proxy artifact for your own deploy pipeline".to_string(),
            }],
            safe_to_retry: false,
        });
    }
    Ok(())
}

/// Write an artifact 0600 and describe it path+bytes only — NEVER content (it
/// carries uuids, and the key-name redactor can't mask a raw JSON string).
fn write_artifact(dir: &Path, name: &str, content: &str) -> Result<serde_json::Value, CliError> {
    let path = dir.join(name);
    std::fs::write(&path, content)
        .map_err(|e| CliError::env("IO_ERROR", format!("{}: {e}", path.display())))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(json!({ "path": path.display().to_string(), "bytes": content.len() }))
}

fn cmd_render(path: &Path, state_dir: &Path, out: Option<PathBuf>) -> Result<String, CliError> {
    let out = out.ok_or_else(|| usage("render requires --out DIR"))?;
    let cfg = config::load(path)?;
    let (errors, _) = config::validate(&cfg);
    if !errors.is_empty() {
        return Err(invalid_config(path, errors));
    }
    std::fs::create_dir_all(&out)
        .map_err(|e| CliError::env("IO_ERROR", format!("{}: {e}", out.display())))?;
    let files = match cfg.mode {
        config::Mode::Proxy => {
            let outbounds = plan::proxy_outbounds(&cfg, state_dir);
            vec![write_artifact(
                &out,
                "sing-box.json",
                &render::render_proxy_sing_box(&cfg, &outbounds),
            )?]
        }
        config::Mode::Gateway => {
            let resolved = subscription::load_resolved(state_dir);
            vec![
                write_artifact(
                    &out,
                    "sing-box.json",
                    &render::render_sing_box(&cfg, resolved.as_ref()),
                )?,
                write_artifact(&out, "nft.rules", &render::render_nft(&cfg))?,
            ]
        }
    };
    Ok(ok_envelope(
        json!({ "out": out.display().to_string(), "files": files }),
    ))
}

fn cmd_resolve(
    config_path: Option<PathBuf>,
    state_dir: &Path,
    url_flag: Option<String>,
    file: Option<&Path>,
    active_flag: Option<String>,
) -> Result<String, CliError> {
    // url/active/strategy come from --url/--active/--config; --file = offline body.
    let sub = match &config_path {
        Some(p) => config::load(p)?.subscription,
        None => None,
    };
    let url = url_flag
        .or_else(|| sub.as_ref().map(|s| s.url.clone()))
        .ok_or_else(|| {
            usage("resolve-subscription needs --url or a [subscription] url in --config")
        })?;
    let strategy = sub
        .as_ref()
        .map_or(config::Strategy::Pinned, |s| s.strategy);
    // --active overrides config (CLI wins).
    let active = active_flag.or_else(|| sub.as_ref().and_then(|s| s.active.clone()));

    let body = match file {
        Some(f) => std::fs::read_to_string(f)
            .map_err(|e| CliError::env("FILE_READ_FAILED", format!("{}: {e}", f.display())))?,
        None => subscription::RealFetcher
            .get(&url)
            .map_err(|e| CliError::env("SUBSCRIPTION_FETCH_FAILED", e))?,
    };
    let parsed = subscription::parse_subscription(&body).map_err(|e| CliError {
        exit: 1,
        code: "SUBSCRIPTION_PARSE_FAILED",
        message: e.0,
        details: Vec::new(),
        suggestions: Vec::new(),
        safe_to_retry: false,
    })?;
    let available: Vec<&str> = parsed.outbounds.iter().map(|o| o.name.as_str()).collect();
    // Never a silent cap: name every node we recognised but did not emit.
    let skipped: Vec<serde_json::Value> = parsed
        .skipped
        .iter()
        .map(|s| json!({ "name": s.name, "protocol": s.scheme, "reason": s.reason }))
        .collect();

    if strategy == config::Strategy::Urltest {
        // urltest uses the whole pool; cache all nodes (v2).
        subscription::save_cache_all(state_dir, &url, None, &parsed.outbounds)
            .map_err(|e| CliError::env("CACHE_WRITE_FAILED", e.to_string()))?;
        return Ok(ok_envelope(json!({
            "source": redact::redact_url(&url),
            "strategy": "urltest",
            "available": available,
            "skipped_unsupported": skipped,
            "resolved": true,
            "outbound_count": parsed.outbounds.len(),
        })));
    }

    // Pinned: without an active name we can only list what's on offer.
    let Some(active) = active else {
        return Ok(ok_envelope(json!({
            "source": redact::redact_url(&url),
            "strategy": "pinned",
            "available": available,
            "skipped_unsupported": skipped,
            "resolved": false,
            "hint": "set [subscription].active or pass --active NAME to pick one",
        })));
    };
    let chosen = subscription::select(&parsed.outbounds, &active).map_err(|e| CliError {
        exit: 1,
        code: "ACTIVE_OUTBOUND_NOT_FOUND",
        message: e.0,
        details: Vec::new(),
        suggestions: Vec::new(),
        safe_to_retry: false,
    })?;
    subscription::save_cache(state_dir, &url, chosen)
        .map_err(|e| CliError::env("CACHE_WRITE_FAILED", e.to_string()))?;
    Ok(ok_envelope(json!({
        "source": redact::redact_url(&url),
        "strategy": "pinned",
        "active": chosen.name,
        "available": available,
        "skipped_unsupported": skipped,
        "resolved": true,
        "outbound": redact::redact_value(&chosen.outbound),
    })))
}

fn cmd_status(config_path: Option<PathBuf>, state_dir: &Path) -> Result<String, CliError> {
    // --config optional: with it status also reports config drift.
    let cfg = match config_path {
        Some(p) => Some(config::load(&p)?),
        None => None,
    };
    status::cmd_status(cfg.as_ref(), state_dir)
}

fn cmd_doctor(path: &Path, state_dir: &Path) -> Result<String, CliError> {
    let cfg = config::load(path)?;
    refuse_if_proxy(&cfg)?;
    let (errors, warnings) = config::validate(&cfg);
    if !errors.is_empty() {
        return Err(invalid_config(path, errors));
    }
    status::cmd_doctor(&cfg, &warnings, state_dir)
}

fn cmd_explain(
    path: &Path,
    source: Option<String>,
    dest: Option<String>,
    proto: Option<String>,
    port: Option<String>,
) -> Result<String, CliError> {
    let cfg = config::load(path)?;
    refuse_if_proxy(&cfg)?;
    let (errors, _) = config::validate(&cfg);
    if !errors.is_empty() {
        return Err(invalid_config(path, errors));
    }
    let source: std::net::IpAddr = source
        .ok_or_else(|| usage("explain requires --source IP"))?
        .parse()
        .map_err(|_| usage("--source is not a valid IP address"))?;
    let dest = match dest {
        Some(d) => Some(
            d.parse::<std::net::IpAddr>()
                .map_err(|_| usage("--dest is not a valid IP address"))?,
        ),
        None => None,
    };
    let proto = match proto.as_deref() {
        None => None,
        Some("tcp") => Some(config::Protocol::Tcp),
        Some("udp") => Some(config::Protocol::Udp),
        Some(other) => {
            return Err(usage(&format!(
                "--proto must be tcp or udp, got \"{other}\""
            )))
        }
    };
    let port = match port {
        Some(p) => Some(
            p.parse::<u16>()
                .map_err(|_| usage("--port must be 1-65535"))?,
        ),
        None => None,
    };
    Ok(plan::explain(&cfg, source, dest, proto, port))
}

fn cmd_apply(
    path: &Path,
    state_dir: &Path,
    yes: bool,
    allow_ssh_risk: bool,
) -> Result<String, CliError> {
    let cfg = config::load(path)?;
    refuse_if_proxy(&cfg)?;
    let (errors, warnings) = config::validate(&cfg);
    if !errors.is_empty() {
        return Err(invalid_config(path, errors));
    }
    let ssh = std::env::var("SSH_CONNECTION")
        .or_else(|_| std::env::var("SSH_CLIENT"))
        .ok();
    apply::run(
        &cfg,
        &warnings,
        path,
        state_dir,
        ssh.as_deref(),
        apply::Opts {
            confirmed: yes,
            allow_ssh_risk,
        },
        &mut apply::RealNft,
        &mut apply::RealDataPlane,
    )
}

fn usage(message: &str) -> CliError {
    CliError::env("USAGE", message.to_string())
}

fn need_config(path: Option<PathBuf>) -> Result<PathBuf, CliError> {
    path.ok_or_else(|| CliError {
        exit: 2,
        code: "CONFIG_PATH_REQUIRED",
        message: "this command requires --config PATH".to_string(),
        details: Vec::new(),
        suggestions: vec![Suggestion {
            command: "vpnrouter-gateway check --config /etc/vpnrouter/gateway.toml --json"
                .to_string(),
            reason: "Pass the gateway.toml path explicitly".to_string(),
        }],
        safe_to_retry: true,
    })
}

fn invalid_config(path: &Path, errors: Vec<config::Finding>) -> CliError {
    CliError {
        exit: 1,
        code: "CONFIG_INVALID",
        message: format!(
            "{} failed validation with {} error(s)",
            path.display(),
            errors.len()
        ),
        details: errors
            .into_iter()
            .map(|f| Detail {
                code: f.code.to_string(),
                message: f.message,
            })
            .collect(),
        suggestions: vec![Suggestion {
            command: "vpnrouter-gateway schema --json".to_string(),
            reason: "Inspect the expected gateway.toml schema".to_string(),
        }],
        safe_to_retry: true,
    }
}

fn cmd_schema() -> Result<String, CliError> {
    let schema: serde_json::Value =
        serde_json::from_str(SCHEMA).expect("embedded schema is valid JSON");
    Ok(ok_envelope(schema))
}

fn cmd_check(path: &Path) -> Result<String, CliError> {
    let cfg = config::load(path)?;
    let (errors, warnings) = config::validate(&cfg);
    if !errors.is_empty() {
        return Err(invalid_config(path, errors));
    }
    Ok(ok_envelope(json!({
        "config_path": path.display().to_string(),
        "mode": match cfg.mode { config::Mode::Proxy => "proxy", config::Mode::Gateway => "gateway" },
        "policies": cfg.policies.len(),
        "management_sources": cfg.management_sources().len(),
        "warnings": warnings,
    })))
}

fn cmd_plan(path: &Path, state_dir: &Path) -> Result<String, CliError> {
    let cfg = config::load(path)?;
    let (errors, warnings) = config::validate(&cfg);
    if !errors.is_empty() {
        return Err(invalid_config(path, errors));
    }
    let ssh = std::env::var("SSH_CONNECTION")
        .or_else(|_| std::env::var("SSH_CLIENT"))
        .ok();
    Ok(plan::build_plan(
        &cfg,
        &warnings,
        path,
        state_dir,
        ssh.as_deref(),
    ))
}
