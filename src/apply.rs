//! apply / rollback: the only mutating commands. Safety order is the contract:
//!   1. candidate nft rules AND sing-box config validated BEFORE any change;
//!   2. current artifacts backed up to last-good BEFORE being replaced;
//!   3. kernel load failure restores artifacts and reloads the previous rules.
//!
//! Apply converges: it always (re)loads its nft table even when artifacts are
//! unchanged, so a reboot-wiped table is repaired by a plain re-apply. The
//! sing-box service is restarted best-effort and reported — no health-probe
//! loop or failover (that lesson from the desktop HealthMonitor stays deferred
//! to a future daemon).

use std::path::Path;

use serde_json::json;

use crate::config::{Finding, GatewayConfig};
use crate::error::{ok_envelope, CliError, Detail, Suggestion};
use crate::{plan, render, subscription};

const ARTIFACTS: [&str; 2] = ["sing-box.json", "nft.rules"];
const SING_BOX_UNIT: &str = "vpnrouter-sing-box";

pub struct Opts {
    pub confirmed: bool,
    pub allow_ssh_risk: bool,
}

/// sing-box config validation outcome. A present-but-rejecting binary is a
/// hard gate (catches the reality short_id panic class); an absent binary is
/// a reported skip (render is unit-tested; owner may run sing-box elsewhere).
pub enum SingBoxCheck {
    Ok,
    Rejected(String),
    Unavailable(String),
}

pub enum RestartOutcome {
    Restarted,
    NotManaged(String),
    Failed(String),
}

/// Data-plane seam (sing-box binary + its service); tests replace it.
pub trait DataPlane {
    fn check_config(&mut self, config: &Path) -> SingBoxCheck;
    fn restart(&mut self) -> RestartOutcome;
}

pub struct RealDataPlane;

impl DataPlane for RealDataPlane {
    fn check_config(&mut self, config: &Path) -> SingBoxCheck {
        match std::process::Command::new("sing-box")
            .arg("check")
            .arg("-c")
            .arg(config)
            .output()
        {
            Err(e) => SingBoxCheck::Unavailable(format!("sing-box not runnable: {e}")),
            Ok(o) if o.status.success() => SingBoxCheck::Ok,
            Ok(o) => SingBoxCheck::Rejected(String::from_utf8_lossy(&o.stderr).trim().to_string()),
        }
    }

    fn restart(&mut self) -> RestartOutcome {
        match std::process::Command::new("systemctl")
            .args(["restart", SING_BOX_UNIT])
            .output()
        {
            Err(e) => RestartOutcome::NotManaged(format!("systemctl not runnable: {e}")),
            Ok(o) if o.status.success() => RestartOutcome::Restarted,
            Ok(o) => {
                let msg = String::from_utf8_lossy(&o.stderr).trim().to_string();
                // Unit simply not installed yet is "not managed", not a failure.
                if msg.contains("not found")
                    || msg.contains("not-found")
                    || msg.contains("No such file")
                {
                    RestartOutcome::NotManaged(format!("unit {SING_BOX_UNIT} not installed"))
                } else {
                    RestartOutcome::Failed(msg)
                }
            }
        }
    }
}

#[derive(Debug)]
pub enum NftError {
    /// nft binary missing/unrunnable — environment problem, nothing mutated.
    NotFound(String),
    /// nft rejected the transaction — stderr attached.
    Failed(String),
}

/// Shell-out boundary; the only seam tests replace.
pub trait NftExec {
    fn check(&mut self, rules: &Path) -> Result<(), NftError>;
    fn load(&mut self, rules: &Path) -> Result<(), NftError>;
}

pub struct RealNft;

impl NftExec for RealNft {
    fn check(&mut self, rules: &Path) -> Result<(), NftError> {
        run_nft(&["-c", "-f"], rules)
    }
    fn load(&mut self, rules: &Path) -> Result<(), NftError> {
        run_nft(&["-f"], rules)
    }
}

fn run_nft(args: &[&str], rules: &Path) -> Result<(), NftError> {
    let out = std::process::Command::new("nft")
        .args(args)
        .arg(rules)
        .output()
        .map_err(|e| NftError::NotFound(format!("cannot run nft: {e}")))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(NftError::Failed(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ))
    }
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    cfg: &GatewayConfig,
    warnings: &[Finding],
    config_path: &Path,
    state_dir: &Path,
    ssh_connection: Option<&str>,
    opts: Opts,
    nft: &mut dyn NftExec,
    dp: &mut dyn DataPlane,
) -> Result<String, CliError> {
    if !opts.confirmed {
        return Err(confirm_required("apply"));
    }
    let assessment = plan::assess(cfg, warnings, state_dir, ssh_connection);
    if let Some(r) = assessment.risks.iter().find(|r| r.code == "SSH_MAY_DROP") {
        if !opts.allow_ssh_risk {
            return Err(CliError {
                exit: 3,
                code: "SSH_RISK_REFUSED",
                message: format!("{} — pass --allow-ssh-risk to apply anyway", r.message),
                details: Vec::new(),
                suggestions: vec![Suggestion {
                    command: "vpnrouter-gateway plan --config /etc/vpnrouter/gateway.toml --json"
                        .to_string(),
                    reason: "Review the full plan and its risks first".to_string(),
                }],
                safe_to_retry: true,
            });
        }
    }

    let resolved = subscription::load_resolved(state_dir);
    let rendered_sing_box = render::render_sing_box(cfg, resolved.as_ref());
    let rendered_nft = render::render_nft(cfg);
    let current = state_dir.join("current");
    let last_good = state_dir.join("last-good");
    std::fs::create_dir_all(&current).map_err(io_err(&current))?;

    // 1. Validate BOTH candidates before touching any state.
    let cand_nft = state_dir.join("candidate.nft.rules");
    std::fs::write(&cand_nft, &rendered_nft).map_err(io_err(&cand_nft))?;
    let nft_checked = nft.check(&cand_nft);
    let cand_sb = state_dir.join("candidate.sing-box.json");
    std::fs::write(&cand_sb, &rendered_sing_box).map_err(io_err(&cand_sb))?;
    let sb_checked = dp.check_config(&cand_sb);
    let _ = std::fs::remove_file(&cand_nft);
    let _ = std::fs::remove_file(&cand_sb);
    nft_checked.map_err(|e| nft_err("NFT_CHECK_FAILED", e))?;
    let mut notes: Vec<String> = Vec::new();
    match sb_checked {
        SingBoxCheck::Ok => {}
        SingBoxCheck::Rejected(stderr) => {
            return Err(CliError {
                exit: 4,
                code: "SINGBOX_CHECK_FAILED",
                message: "sing-box rejected the rendered config; nothing was changed".to_string(),
                details: vec![Detail {
                    code: "SINGBOX_STDERR".to_string(),
                    message: stderr,
                }],
                suggestions: Vec::new(),
                safe_to_retry: true,
            });
        }
        SingBoxCheck::Unavailable(why) => {
            notes.push(format!("sing-box config not validated: {why}"));
        }
    }
    if resolved.is_none() {
        notes.push("vpn outbound is a placeholder; run resolve-subscription".to_string());
    }

    // 2. Backup current artifacts before replacing them.
    let file_change = !assessment.changes.is_empty();
    let mut backed_up = false;
    if file_change && ARTIFACTS.iter().all(|f| current.join(f).exists()) {
        std::fs::create_dir_all(&last_good).map_err(io_err(&last_good))?;
        for f in ARTIFACTS {
            std::fs::copy(current.join(f), last_good.join(f)).map_err(io_err(&last_good))?;
        }
        backed_up = true;
    }

    // 3. Commit artifacts (write + rename per file).
    write_artifact(&current, "sing-box.json", &rendered_sing_box)?;
    write_artifact(&current, "nft.rules", &rendered_nft)?;

    // 4. Load into the kernel; on failure restore what we replaced.
    if let Err(e) = nft.load(&current.join("nft.rules")) {
        let restored = if backed_up {
            for f in ARTIFACTS {
                let _ = std::fs::copy(last_good.join(f), current.join(f));
            }
            match nft.load(&current.join("nft.rules")) {
                Ok(()) => "previous artifacts restored and reloaded",
                Err(_) => "previous artifacts restored but reload FAILED — inspect nft state",
            }
        } else if file_change {
            for f in ARTIFACTS {
                let _ = std::fs::remove_file(current.join(f));
            }
            "new artifacts removed; kernel untouched"
        } else {
            "artifacts unchanged"
        };
        let mut err = nft_err("APPLY_FAILED_ROLLED_BACK", e);
        err.message = format!("nft load failed; {restored}");
        return Err(err);
    }

    // 5. Best-effort data-plane restart (reported, never rolls back nft).
    let (service, service_detail) = match dp.restart() {
        RestartOutcome::Restarted => ("restarted", None),
        RestartOutcome::NotManaged(why) => ("not-managed", Some(why)),
        RestartOutcome::Failed(why) => ("restart-failed", Some(why)),
    };

    Ok(ok_envelope(json!({
        "config_path": config_path.display().to_string(),
        "changes": assessment.changes,
        "risks": assessment.risks,
        "backed_up": backed_up,
        "nft_loaded": true,
        "outbound_resolved": resolved.is_some(),
        "sing_box_service": service,
        "sing_box_service_detail": service_detail,
        "notes": notes,
    })))
}

pub fn rollback(
    state_dir: &Path,
    confirmed: bool,
    nft: &mut dyn NftExec,
) -> Result<String, CliError> {
    if !confirmed {
        return Err(confirm_required("rollback"));
    }
    let last_good = state_dir.join("last-good");
    let current = state_dir.join("current");
    if !ARTIFACTS.iter().all(|f| last_good.join(f).exists()) {
        return Err(CliError {
            exit: 2,
            code: "NO_LAST_GOOD",
            message:
                "no last-good state to roll back to (apply has never replaced existing artifacts)"
                    .to_string(),
            details: Vec::new(),
            suggestions: Vec::new(),
            safe_to_retry: false,
        });
    }
    nft.check(&last_good.join("nft.rules"))
        .map_err(|e| nft_err("NFT_CHECK_FAILED", e))?;
    std::fs::create_dir_all(&current).map_err(io_err(&current))?;
    let mut restored = Vec::new();
    for f in ARTIFACTS {
        std::fs::copy(last_good.join(f), current.join(f)).map_err(io_err(&current))?;
        restored.push(current.join(f).display().to_string());
    }
    if let Err(e) = nft.load(&current.join("nft.rules")) {
        let mut err = nft_err("ROLLBACK_LOAD_FAILED", e);
        err.message =
            "last-good artifacts restored but nft load failed — kernel state may not match artifacts"
                .to_string();
        return Err(err);
    }
    Ok(ok_envelope(json!({
        "restored": restored,
        "nft_loaded": true,
    })))
}

fn confirm_required(what: &str) -> CliError {
    CliError {
        exit: 3,
        code: "CONFIRM_REQUIRED",
        message: format!("{what} mutates host state; re-run with --yes"),
        details: Vec::new(),
        suggestions: vec![Suggestion {
            command: "vpnrouter-gateway plan --config /etc/vpnrouter/gateway.toml --json"
                .to_string(),
            reason: "Review what would change before confirming".to_string(),
        }],
        safe_to_retry: true,
    }
}

fn nft_err(code: &'static str, e: NftError) -> CliError {
    match e {
        NftError::NotFound(msg) => CliError::env("NFT_NOT_FOUND", msg),
        NftError::Failed(stderr) => CliError {
            exit: 4,
            code,
            message: "nft rejected the ruleset".to_string(),
            details: vec![Detail {
                code: "NFT_STDERR".to_string(),
                message: stderr,
            }],
            suggestions: Vec::new(),
            safe_to_retry: true,
        },
    }
}

fn io_err(path: &Path) -> impl Fn(std::io::Error) -> CliError + '_ {
    move |e| CliError::env("IO_ERROR", format!("{}: {e}", path.display()))
}

fn write_artifact(dir: &Path, name: &str, content: &str) -> Result<(), CliError> {
    let tmp = dir.join(format!("{name}.tmp"));
    std::fs::write(&tmp, content).map_err(io_err(dir))?;
    std::fs::rename(&tmp, dir.join(name)).map_err(io_err(dir))
}
