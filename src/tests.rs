//! Spike 0 test suite: parse -> validate -> render -> plan, plus the two
//! invariants the whole product stands on: deterministic render and
//! own-table-only nft output.

use std::path::{Path, PathBuf};

use serde_json::json;

use crate::{config, plan, render};

const SAMPLE: &str = include_str!("../examples/gateway.toml");
const GOLDEN_SING_BOX: &str = include_str!("../tests/golden/sing-box.json");
const GOLDEN_NFT: &str = include_str!("../tests/golden/nft.rules");

fn norm(s: &str) -> String {
    s.replace("\r\n", "\n")
}

fn sample() -> config::GatewayConfig {
    toml::from_str(&norm(SAMPLE)).expect("sample config parses")
}

fn validate_str(s: &str) -> (Vec<config::Finding>, Vec<config::Finding>) {
    let cfg: config::GatewayConfig = toml::from_str(s).expect("test config parses");
    config::validate(&cfg)
}

fn tmpdir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("vpnr-spike-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("current")).expect("create temp state dir");
    d
}

#[test]
fn sample_parses_and_validates_clean() {
    let cfg = sample();
    assert_eq!(cfg.policies.len(), 2);
    assert_eq!(cfg.policies[0].name, "voice-udp");
    assert_eq!(cfg.management_sources().len(), 1);
    assert!(cfg.killswitch_enabled());
    let (errors, warnings) = config::validate(&cfg);
    assert!(errors.is_empty(), "{errors:?}");
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn wan_lan_same_is_error() {
    let s = norm(SAMPLE).replace("wan = \"eth0\"", "wan = \"br0\"");
    let (errors, _) = validate_str(&s);
    assert!(
        errors.iter().any(|e| e.code == "WAN_LAN_SAME"),
        "{errors:?}"
    );
}

#[test]
fn port_without_protocol_is_error() {
    let s = norm(SAMPLE).replace("protocol = \"udp\"\n", "");
    let (errors, _) = validate_str(&s);
    assert!(
        errors.iter().any(|e| e.code == "PORT_WITHOUT_PROTOCOL"),
        "{errors:?}"
    );
}

#[test]
fn duplicate_policy_name_is_error() {
    let s = norm(SAMPLE).replace("name = \"voice-udp\"", "name = \"office-default\"");
    let (errors, _) = validate_str(&s);
    assert!(
        errors.iter().any(|e| e.code == "DUPLICATE_POLICY_NAME"),
        "{errors:?}"
    );
}

#[test]
fn mtu_out_of_range_is_error() {
    let s = norm(SAMPLE).replace("mtu = 1420", "mtu = 9000");
    let (errors, _) = validate_str(&s);
    assert!(
        errors.iter().any(|e| e.code == "MTU_OUT_OF_RANGE"),
        "{errors:?}"
    );
}

#[test]
fn killswitch_without_vpn_traffic_is_error() {
    let s = norm(SAMPLE)
        .replace("route = \"vpn\"", "route = \"direct\"")
        .replace("mode = \"full\"", "mode = \"split\"");
    let (errors, _) = validate_str(&s);
    assert!(
        errors
            .iter()
            .any(|e| e.code == "KILLSWITCH_WITHOUT_VPN_POLICY"),
        "{errors:?}"
    );
}

#[test]
fn missing_management_is_warning() {
    let s = norm(SAMPLE).replace("sources = [\"192.168.10.50/32\"]", "sources = []");
    let (errors, warnings) = validate_str(&s);
    assert!(errors.is_empty(), "{errors:?}");
    assert!(
        warnings.iter().any(|w| w.code == "NO_MANAGEMENT_BYPASS"),
        "{warnings:?}"
    );
}

#[test]
fn shadowed_policy_is_warning() {
    // office-default (broad, no proto) placed before voice-udp: first-match
    // makes voice-udp dead code.
    let s = r#"
[interfaces]
wan = "eth0"
lan = "br0"
lan_cidr = "192.168.10.0/24"

[management]
sources = ["192.168.10.50/32"]

[routing]
mode = "full"

[[policies]]
name = "office-default"
source = "192.168.10.0/24"
route = "vpn"

[[policies]]
name = "voice-udp"
source = "192.168.10.0/24"
protocol = "udp"
port = 50000
route = "vpn"

[killswitch]
enabled = true
"#;
    let (errors, warnings) = validate_str(s);
    assert!(errors.is_empty(), "{errors:?}");
    let shadow = warnings
        .iter()
        .find(|w| w.code == "POLICY_SHADOWED")
        .expect("shadow warning");
    assert!(shadow.message.contains("voice-udp"), "{}", shadow.message);
}

#[test]
fn unknown_field_is_parse_error() {
    let s = format!("{}\nbogus_field = 1\n", norm(SAMPLE));
    assert!(toml::from_str::<config::GatewayConfig>(&s).is_err());
}

#[test]
fn render_is_deterministic_and_matches_golden() {
    let cfg = sample();
    let sb = render::render_sing_box(&cfg, None);
    let nft = render::render_nft(&cfg);
    assert_eq!(
        sb,
        render::render_sing_box(&cfg, None),
        "sing-box render is not deterministic"
    );
    assert_eq!(
        nft,
        render::render_nft(&cfg),
        "nft render is not deterministic"
    );
    if std::env::var("UPDATE_GOLDEN").is_ok() {
        std::fs::write("tests/golden/sing-box.json", &sb).unwrap();
        std::fs::write("tests/golden/nft.rules", &nft).unwrap();
    }
    assert_eq!(
        norm(&sb),
        norm(GOLDEN_SING_BOX),
        "sing-box render drifted (UPDATE_GOLDEN=1 cargo test to regenerate)"
    );
    assert_eq!(
        norm(&nft),
        norm(GOLDEN_NFT),
        "nft render drifted (UPDATE_GOLDEN=1 cargo test to regenerate)"
    );
}

#[test]
fn sing_box_render_shape() {
    let v: serde_json::Value =
        serde_json::from_str(&render::render_sing_box(&sample(), None)).unwrap();
    assert_eq!(v["route"]["final"], "vpn-out");
    assert_eq!(v["dns"]["final"], "vpn-dns");
    assert_eq!(v["dns"]["strategy"], "ipv4_only");
    assert_eq!(v["inbounds"][0]["mtu"], 1420);
    assert_eq!(v["inbounds"][0]["stack"], "system");
    assert_eq!(
        v["inbounds"][0]["route_exclude_address"][0],
        "192.168.10.50/32"
    );
    let rules = v["route"]["rules"].as_array().unwrap();
    // 3 fixed rules, then management, then policies in file order.
    assert_eq!(rules[3]["source_ip_cidr"][0], "192.168.10.50/32");
    assert_eq!(rules[3]["outbound"], "direct");
    assert_eq!(rules[4]["port"][0], 50000);
    assert_eq!(rules[4]["network"], "udp");
    assert_eq!(rules[4]["outbound"], "vpn-out");
    assert_eq!(rules[5]["outbound"], "vpn-out");
}

#[test]
fn nft_only_touches_own_table() {
    let nft = render::render_nft(&sample());
    let table_lines: Vec<&str> = nft
        .lines()
        .filter(|l| {
            let t = l.trim_start();
            t.starts_with("table ") || t.starts_with("delete table")
        })
        .collect();
    assert_eq!(table_lines.len(), 3, "{table_lines:?}");
    for l in &table_lines {
        assert!(l.contains("inet vpnrouter"), "foreign table reference: {l}");
    }
    assert!(!nft.contains("flush ruleset"));
    assert!(nft.contains("ip saddr 192.168.10.0/24"));
    assert!(nft.contains("udp dport 50000"));
    assert!(nft.contains("masquerade"));
    assert!(nft.contains("meta nfproto ipv6 drop"));
    assert!(nft.contains("iifname \"br0\" ip saddr 192.168.10.50/32 accept"));
}

#[test]
fn plan_on_empty_state_reports_creates_and_risks() {
    let cfg = sample();
    let (errors, warnings) = config::validate(&cfg);
    assert!(errors.is_empty());
    let d = tmpdir("empty");
    let out = plan::build_plan(
        &cfg,
        &warnings,
        Path::new("examples/gateway.toml"),
        &d,
        None,
    );
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["ok"], true);
    let changes = v["changes"].as_array().unwrap();
    assert_eq!(changes.len(), 2, "{changes:?}");
    assert!(changes
        .iter()
        .any(|c| c["target"] == "sing-box" && c["action"] == "create"));
    assert!(changes
        .iter()
        .any(|c| c["target"] == "nftables" && c["action"] == "create"));
    let risks = v["risks"].as_array().unwrap();
    assert!(
        risks.iter().any(|r| r["code"] == "OUTBOUND_UNRESOLVED"),
        "{risks:?}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn plan_is_empty_when_current_matches() {
    let cfg = sample();
    let d = tmpdir("match");
    std::fs::write(
        d.join("current").join("sing-box.json"),
        render::render_sing_box(&cfg, None),
    )
    .unwrap();
    std::fs::write(
        d.join("current").join("nft.rules"),
        render::render_nft(&cfg),
    )
    .unwrap();
    let out = plan::build_plan(&cfg, &[], Path::new("x"), &d, None);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["changes"].as_array().unwrap().len(), 0);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn ssh_risk_detection() {
    let cfg = sample();
    // management host: safe
    assert!(plan::ssh_risk(&cfg, Some("192.168.10.50 51000 192.168.10.1 22")).is_none());
    // ordinary LAN host: matched by office-default (vpn, tcp-capable) -> risk
    let msg = plan::ssh_risk(&cfg, Some("192.168.10.77 51000 192.168.10.1 22")).expect("risk");
    assert!(msg.contains("office-default"), "{msg}");
    // no SSH env: no risk
    assert!(plan::ssh_risk(&cfg, None).is_none());
    // garbage env: no panic, no risk
    assert!(plan::ssh_risk(&cfg, Some("not-an-ip")).is_none());
}

#[test]
fn embedded_schema_is_valid_json() {
    let v: serde_json::Value = serde_json::from_str(crate::SCHEMA).unwrap();
    assert_eq!(v["$schema"], "https://json-schema.org/draft/2020-12/schema");
    assert!(v["properties"]["policies"].is_object());
}

#[test]
fn parses_ip_j_addr_output() {
    let canned = r#"[
        {"ifindex":1,"ifname":"lo","operstate":"UNKNOWN","addr_info":[{"family":"inet","local":"127.0.0.1","prefixlen":8}]},
        {"ifindex":2,"ifname":"eth0","operstate":"UP","addr_info":[{"family":"inet","local":"192.0.2.10","prefixlen":24}]}
    ]"#;
    let ifs = crate::status::parse_ip_addr(canned).unwrap();
    assert_eq!(ifs.len(), 2);
    assert_eq!(ifs[1].name, "eth0");
    assert_eq!(ifs[1].state, "UP");
    assert_eq!(ifs[1].addresses, vec!["192.0.2.10/24".to_string()]);
}

// ---------- explain ----------

fn explain_verdict(
    cfg: &config::GatewayConfig,
    source: &str,
    proto: Option<config::Protocol>,
    port: Option<u16>,
) -> serde_json::Value {
    let out = plan::explain(cfg, source.parse().unwrap(), None, proto, port);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    v["data"]["verdict"].clone()
}

#[test]
fn explain_management_source_is_direct() {
    let v = explain_verdict(&sample(), "192.168.10.50", None, None);
    assert_eq!(v["route"], "direct");
    assert_eq!(v["decided_by"]["via"], "management");
}

#[test]
fn explain_matches_specific_udp_policy() {
    let v = explain_verdict(
        &sample(),
        "192.168.10.77",
        Some(config::Protocol::Udp),
        Some(50000),
    );
    assert_eq!(v["route"], "vpn");
    assert_eq!(v["decided_by"]["name"], "voice-udp");
}

#[test]
fn explain_falls_through_proto_policy_to_broad_one() {
    let v = explain_verdict(
        &sample(),
        "192.168.10.77",
        Some(config::Protocol::Tcp),
        Some(443),
    );
    assert_eq!(v["route"], "vpn");
    assert_eq!(v["decided_by"]["name"], "office-default");
}

#[test]
fn explain_unmatched_source_uses_routing_mode() {
    let v = explain_verdict(&sample(), "10.9.9.9", None, None);
    assert_eq!(v["route"], "vpn");
    assert_eq!(v["decided_by"]["via"], "routing.mode=full");
}

#[test]
fn explain_block_policy_rejects() {
    let s = norm(SAMPLE).replace(
        "name = \"voice-udp\"\nsource = \"192.168.10.0/24\"\nprotocol = \"udp\"\nport = 50000\nroute = \"vpn\"",
        "name = \"no-iot\"\nsource = \"192.168.10.200/32\"\nroute = \"block\"",
    );
    let cfg: config::GatewayConfig = toml::from_str(&s).unwrap();
    let v = explain_verdict(&cfg, "192.168.10.200", None, None);
    assert_eq!(v["route"], "block");
    assert_eq!(v["outbound"], "reject");
    assert_eq!(v["decided_by"]["name"], "no-iot");
}

// ---------- status / doctor (pure parts) ----------

#[test]
fn status_artifact_flags_and_sync() {
    let cfg = sample();
    let d = tmpdir("status");
    let files: &[&str] = &["sing-box.json", "nft.rules"];
    assert_eq!(crate::status::artifact_flags(&d, files), (false, false));
    assert_eq!(crate::status::config_in_sync(&cfg, &d), None);
    do_apply(&cfg, &d, None, yes(), &mut FakeNft::default()).unwrap();
    assert_eq!(crate::status::artifact_flags(&d, files), (true, false));
    assert_eq!(crate::status::config_in_sync(&cfg, &d), Some(true));
    // edit config -> drift
    let changed: config::GatewayConfig =
        toml::from_str(&norm(SAMPLE).replace("port = 50000", "port = 60000")).unwrap();
    assert_eq!(crate::status::config_in_sync(&changed, &d), Some(false));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn doctor_pure_checks_flag_missing_artifacts_and_drift() {
    let cfg = sample();
    let d = tmpdir("doctor");
    let checks = crate::status::pure_doctor_checks(&cfg, &[], &d);
    assert!(checks
        .iter()
        .any(|c| c.name == "artifacts" && c.level == "warning" && c.message.contains("apply")));
    do_apply(&cfg, &d, None, yes(), &mut FakeNft::default()).unwrap();
    let checks = crate::status::pure_doctor_checks(&cfg, &[], &d);
    assert!(checks
        .iter()
        .any(|c| c.name == "artifacts" && c.level == "ok"));
    let changed: config::GatewayConfig =
        toml::from_str(&norm(SAMPLE).replace("port = 50000", "port = 60000")).unwrap();
    let checks = crate::status::pure_doctor_checks(&changed, &[], &d);
    assert!(checks
        .iter()
        .any(|c| c.name == "artifacts" && c.level == "warning" && c.message.contains("changed")));
    let _ = std::fs::remove_dir_all(&d);
}

// ---------- apply / rollback ----------

use crate::apply::{self, DataPlane, NftError, NftExec, Opts, RestartOutcome, SingBoxCheck};

#[derive(Default)]
struct FakeNft {
    calls: Vec<String>,
    fail_check: bool,
    fail_load: bool,
}

impl NftExec for FakeNft {
    fn check(&mut self, rules: &Path) -> Result<(), NftError> {
        self.calls.push(format!(
            "check {}",
            rules.file_name().unwrap().to_string_lossy()
        ));
        if self.fail_check {
            Err(NftError::Failed("fake check failure".to_string()))
        } else {
            Ok(())
        }
    }
    fn load(&mut self, rules: &Path) -> Result<(), NftError> {
        self.calls.push(format!(
            "load {}",
            rules.file_name().unwrap().to_string_lossy()
        ));
        if self.fail_load {
            Err(NftError::Failed("fake load failure".to_string()))
        } else {
            Ok(())
        }
    }
}

#[derive(Default)]
struct FakeDataPlane {
    reject: bool,
    restarted: bool,
}

impl DataPlane for FakeDataPlane {
    fn check_config(&mut self, _config: &Path) -> SingBoxCheck {
        if self.reject {
            SingBoxCheck::Rejected("fake sing-box rejection".to_string())
        } else {
            SingBoxCheck::Ok
        }
    }
    fn restart(&mut self) -> RestartOutcome {
        self.restarted = true;
        RestartOutcome::NotManaged("fake: no service".to_string())
    }
}

fn do_apply(
    cfg: &config::GatewayConfig,
    dir: &Path,
    ssh: Option<&str>,
    opts: Opts,
    nft: &mut FakeNft,
) -> Result<String, crate::error::CliError> {
    do_apply_dp(cfg, dir, ssh, opts, nft, &mut FakeDataPlane::default())
}

fn do_apply_dp(
    cfg: &config::GatewayConfig,
    dir: &Path,
    ssh: Option<&str>,
    opts: Opts,
    nft: &mut FakeNft,
    dp: &mut FakeDataPlane,
) -> Result<String, crate::error::CliError> {
    let (errors, warnings) = config::validate(cfg);
    assert!(errors.is_empty(), "{errors:?}");
    apply::run(cfg, &warnings, Path::new("x.toml"), dir, ssh, opts, nft, dp)
}

fn yes() -> Opts {
    Opts {
        confirmed: true,
        allow_ssh_risk: false,
    }
}

#[test]
fn apply_requires_yes() {
    let cfg = sample();
    let d = tmpdir("apply-noyes");
    let mut nft = FakeNft::default();
    let err = do_apply(
        &cfg,
        &d,
        None,
        Opts {
            confirmed: false,
            allow_ssh_risk: false,
        },
        &mut nft,
    )
    .unwrap_err();
    assert_eq!(err.code, "CONFIRM_REQUIRED");
    assert_eq!(err.exit, 3);
    assert!(nft.calls.is_empty());
    assert!(!d.join("current").join("nft.rules").exists());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn apply_refuses_ssh_risk_without_flag() {
    let cfg = sample();
    let d = tmpdir("apply-ssh");
    let mut nft = FakeNft::default();
    let err = do_apply(
        &cfg,
        &d,
        Some("192.168.10.77 51000 192.168.10.1 22"),
        yes(),
        &mut nft,
    )
    .unwrap_err();
    assert_eq!(err.code, "SSH_RISK_REFUSED");
    assert_eq!(err.exit, 3);
    assert!(nft.calls.is_empty());
    assert!(!d.join("current").join("nft.rules").exists());
    // with the flag it proceeds
    let mut nft = FakeNft::default();
    let out = do_apply(
        &cfg,
        &d,
        Some("192.168.10.77 51000 192.168.10.1 22"),
        Opts {
            confirmed: true,
            allow_ssh_risk: true,
        },
        &mut nft,
    )
    .unwrap();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["data"]["nft_loaded"], true);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn apply_first_time_writes_and_loads() {
    let cfg = sample();
    let d = tmpdir("apply-first");
    let mut nft = FakeNft::default();
    let out = do_apply(&cfg, &d, None, yes(), &mut nft).unwrap();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["data"]["backed_up"], false);
    assert_eq!(v["data"]["changes"].as_array().unwrap().len(), 2);
    assert_eq!(
        nft.calls,
        vec!["check candidate.nft.rules", "load nft.rules"]
    );
    assert_eq!(
        std::fs::read_to_string(d.join("current").join("nft.rules")).unwrap(),
        render::render_nft(&cfg)
    );
    assert!(!d.join("candidate.nft.rules").exists());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn apply_converges_when_unchanged() {
    let cfg = sample();
    let d = tmpdir("apply-conv");
    do_apply(&cfg, &d, None, yes(), &mut FakeNft::default()).unwrap();
    let mut nft = FakeNft::default();
    let out = do_apply(&cfg, &d, None, yes(), &mut nft).unwrap();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["data"]["changes"].as_array().unwrap().len(), 0);
    assert_eq!(v["data"]["backed_up"], false);
    // still reloads the kernel table: a reboot-wiped table is repaired
    assert_eq!(v["data"]["nft_loaded"], true);
    assert!(nft.calls.contains(&"load nft.rules".to_string()));
    assert!(!d.join("last-good").exists());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn apply_backs_up_previous_state_on_change() {
    let cfg = sample();
    let d = tmpdir("apply-backup");
    do_apply(&cfg, &d, None, yes(), &mut FakeNft::default()).unwrap();
    let old_nft = render::render_nft(&cfg);
    let changed: config::GatewayConfig =
        toml::from_str(&norm(SAMPLE).replace("port = 50000", "port = 60000")).unwrap();
    let out = do_apply(&changed, &d, None, yes(), &mut FakeNft::default()).unwrap();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["data"]["backed_up"], true);
    assert_eq!(
        std::fs::read_to_string(d.join("last-good").join("nft.rules")).unwrap(),
        old_nft
    );
    assert_eq!(
        std::fs::read_to_string(d.join("current").join("nft.rules")).unwrap(),
        render::render_nft(&changed)
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn apply_check_failure_mutates_nothing() {
    let cfg = sample();
    let d = tmpdir("apply-checkfail");
    let mut nft = FakeNft {
        fail_check: true,
        ..Default::default()
    };
    let err = do_apply(&cfg, &d, None, yes(), &mut nft).unwrap_err();
    assert_eq!(err.code, "NFT_CHECK_FAILED");
    assert_eq!(err.exit, 4);
    assert!(!d.join("current").join("nft.rules").exists());
    assert!(!d.join("current").join("sing-box.json").exists());
    assert!(!nft.calls.contains(&"load nft.rules".to_string()));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn apply_load_failure_restores_previous_artifacts() {
    let cfg = sample();
    let d = tmpdir("apply-loadfail");
    do_apply(&cfg, &d, None, yes(), &mut FakeNft::default()).unwrap();
    let old_nft = render::render_nft(&cfg);
    let changed: config::GatewayConfig =
        toml::from_str(&norm(SAMPLE).replace("port = 50000", "port = 60000")).unwrap();
    let mut nft = FakeNft {
        fail_load: true,
        ..Default::default()
    };
    let err = do_apply(&changed, &d, None, yes(), &mut nft).unwrap_err();
    assert_eq!(err.code, "APPLY_FAILED_ROLLED_BACK");
    assert_eq!(err.exit, 4);
    // current is back to the previous (working) content
    assert_eq!(
        std::fs::read_to_string(d.join("current").join("nft.rules")).unwrap(),
        old_nft
    );
    // restore attempted a reload of the previous rules
    assert_eq!(
        nft.calls
            .iter()
            .filter(|c| c.as_str() == "load nft.rules")
            .count(),
        2
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn rollback_requires_yes_and_last_good() {
    let d = tmpdir("rb-guards");
    let err = apply::rollback(&d, false, &mut FakeNft::default()).unwrap_err();
    assert_eq!(err.code, "CONFIRM_REQUIRED");
    let err = apply::rollback(&d, true, &mut FakeNft::default()).unwrap_err();
    assert_eq!(err.code, "NO_LAST_GOOD");
    assert_eq!(err.exit, 2);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn rollback_restores_last_good() {
    let cfg = sample();
    let d = tmpdir("rb-restore");
    do_apply(&cfg, &d, None, yes(), &mut FakeNft::default()).unwrap();
    let old_nft = render::render_nft(&cfg);
    let changed: config::GatewayConfig =
        toml::from_str(&norm(SAMPLE).replace("port = 50000", "port = 60000")).unwrap();
    do_apply(&changed, &d, None, yes(), &mut FakeNft::default()).unwrap();
    let mut nft = FakeNft::default();
    let out = apply::rollback(&d, true, &mut nft).unwrap();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["data"]["nft_loaded"], true);
    assert_eq!(
        std::fs::read_to_string(d.join("current").join("nft.rules")).unwrap(),
        old_nft
    );
    assert_eq!(nft.calls, vec!["check nft.rules", "load nft.rules"]);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn apply_singbox_check_rejection_mutates_nothing() {
    let cfg = sample();
    let d = tmpdir("apply-sbreject");
    let mut nft = FakeNft::default();
    let mut dp = FakeDataPlane {
        reject: true,
        ..Default::default()
    };
    let err = do_apply_dp(&cfg, &d, None, yes(), &mut nft, &mut dp).unwrap_err();
    assert_eq!(err.code, "SINGBOX_CHECK_FAILED");
    assert_eq!(err.exit, 4);
    assert!(!d.join("current").join("nft.rules").exists());
    assert!(!d.join("current").join("sing-box.json").exists());
    assert!(!nft.calls.contains(&"load nft.rules".to_string()));
    // candidates cleaned up
    assert!(!d.join("candidate.sing-box.json").exists());
    assert!(!d.join("candidate.nft.rules").exists());
    let _ = std::fs::remove_dir_all(&d);
}

// ---------- subscription / redact ----------

use crate::subscription::{self, base64_decode, parse_subscription, select};

/// Test-only base64 encoder, standard alphabet, so fixtures cross-check the
/// decoder without a dependency.
fn b64(data: &str) -> String {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let b = data.as_bytes();
    let mut out = String::new();
    for chunk in b.chunks(3) {
        let n = chunk.len();
        let triple = (chunk[0] as u32) << 16
            | (if n > 1 { chunk[1] as u32 } else { 0 }) << 8
            | (if n > 2 { chunk[2] as u32 } else { 0 });
        out.push(A[(triple >> 18 & 63) as usize] as char);
        out.push(A[(triple >> 12 & 63) as usize] as char);
        out.push(if n > 1 {
            A[(triple >> 6 & 63) as usize] as char
        } else {
            '='
        });
        out.push(if n > 2 {
            A[(triple & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

const VLESS_REALITY: &str = "vless://b831381d-6324-4d53-ad4f-8cda48b30811@server.example:443?encryption=none&security=reality&sni=www.microsoft.com&fp=chrome&pbk=PUBKEY123&sid=1a2b&type=tcp&flow=xtls-rprx-vision#Germany%20VLESS";

#[test]
fn base64_decode_standard_and_urlsafe() {
    assert_eq!(base64_decode("aGVsbG8=").unwrap(), b"hello");
    assert_eq!(base64_decode("aGVsbG8").unwrap(), b"hello"); // no padding
    assert_eq!(base64_decode("PGI-").unwrap(), b"<b>"); // url-safe '-' == standard '+'
    assert_eq!(base64_decode("Pz8_").unwrap(), b"???"); // url-safe '_' == standard '/'
    assert!(base64_decode("not valid !!!").is_none());
    // roundtrip via test encoder
    assert_eq!(base64_decode(&b64("vless://x")).unwrap(), b"vless://x");
}

#[test]
fn parse_vless_reality_share_link() {
    let obs = parse_subscription(VLESS_REALITY).unwrap().outbounds;
    assert_eq!(obs.len(), 1);
    let o = &obs[0];
    assert_eq!(o.name, "Germany VLESS");
    let j = &o.outbound;
    assert_eq!(j["type"], "vless");
    assert_eq!(j["server"], "server.example");
    assert_eq!(j["server_port"], 443);
    assert_eq!(j["uuid"], "b831381d-6324-4d53-ad4f-8cda48b30811");
    assert_eq!(j["flow"], "xtls-rprx-vision");
    assert_eq!(j["packet_encoding"], "xudp");
    assert_eq!(j["tls"]["server_name"], "www.microsoft.com");
    assert_eq!(j["tls"]["utls"]["fingerprint"], "chrome");
    assert_eq!(j["tls"]["reality"]["public_key"], "PUBKEY123");
    assert_eq!(j["tls"]["reality"]["short_id"], "1a2b");
    assert!(j.get("transport").is_none(), "tcp needs no transport block");
}

#[test]
fn parse_base64_wrapped_list_and_select() {
    let list = format!(
        "{VLESS_REALITY}\nvless://22222222-2222-2222-2222-222222222222@b.example:8443?security=tls&sni=b.example#Backup"
    );
    let obs = parse_subscription(&b64(&list)).unwrap().outbounds;
    assert_eq!(obs.len(), 2);
    assert_eq!(select(&obs, "Germany VLESS").unwrap().name, "Germany VLESS");
    assert_eq!(
        select(&obs, "Backup").unwrap().outbound["server"],
        "b.example"
    );
    let err = select(&obs, "Nonexistent").unwrap_err();
    assert!(err.0.contains("available"), "{}", err.0);
}

#[test]
fn parse_json_wrapper_ninitux_shape() {
    // ninitux returns {"status":"ok","config":"<base64 of a vless list>"}
    let wrapped = format!(
        r#"{{"status":"ok","app":"vpn-router","config":"{}"}}"#,
        b64(VLESS_REALITY)
    );
    let obs = parse_subscription(&wrapped).unwrap().outbounds;
    assert_eq!(obs.len(), 1);
    assert_eq!(obs[0].name, "Germany VLESS");
    assert_eq!(obs[0].outbound["server"], "server.example");
}

#[test]
fn parse_singbox_json_passthrough() {
    // JP carries tls so it survives the node-safety filter; the test's point is
    // that proxy outbounds are kept while direct/selector are dropped.
    let json = r#"{"outbounds":[
        {"type":"vless","tag":"JP","server":"jp.example","server_port":443,"uuid":"u","tls":{"enabled":true,"server_name":"jp.example"}},
        {"type":"direct","tag":"direct"},
        {"type":"selector","tag":"select","outbounds":["JP"]}
    ]}"#;
    let obs = parse_subscription(json).unwrap().outbounds;
    assert_eq!(obs.len(), 1, "only proxy outbounds, not direct/selector");
    assert_eq!(obs[0].name, "JP");
}

#[test]
fn parse_unsupported_only_errors() {
    let err = parse_subscription("tuic://x@h:443#H\nss://y@h:443#T").unwrap_err();
    assert!(err.0.contains("no supported outbounds"), "{}", err.0);
}

#[test]
fn skipped_unsupported_nodes_are_named() {
    // vless + hysteria2 supported; tuic/naive skipped but surfaced (no silent cap).
    let list = format!(
        "{VLESS_REALITY}\nhysteria2://p@h:443#Germany%20HY2\ntuic://x@h:443#TU\nnaive+https://x@h:443#LV%20NAIVE"
    );
    let r = parse_subscription(&list).unwrap();
    assert_eq!(r.outbounds.len(), 2, "vless + hysteria2");
    assert_eq!(r.skipped.len(), 2, "tuic + naive");
    assert!(r
        .skipped
        .iter()
        .any(|s| s.scheme == "tuic" && s.name == "TU"));
    assert!(r.skipped.iter().any(|s| s.scheme == "naive+https"));
}

#[test]
fn resolve_cache_roundtrip_and_render() {
    let d = tmpdir("resolve");
    let obs = parse_subscription(VLESS_REALITY).unwrap().outbounds;
    let chosen = select(&obs, "Germany VLESS").unwrap();
    subscription::save_cache(&d, "https://sub.example/token123", chosen).unwrap();
    let resolved = subscription::load_resolved(&d).expect("cache loads");
    assert_eq!(resolved["server"], "server.example");
    // cache redacts the source URL
    let cache: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(d.join("subscription.json")).unwrap())
            .unwrap();
    assert_eq!(cache["source"], "https://sub.example/…");
    // render uses the resolved outbound, retagged to vpn-out
    let cfg = sample();
    let sb: serde_json::Value =
        serde_json::from_str(&render::render_sing_box(&cfg, Some(&resolved))).unwrap();
    assert_eq!(sb["outbounds"][0]["tag"], "vpn-out");
    assert_eq!(sb["outbounds"][0]["server"], "server.example");
    assert_eq!(sb["route"]["final"], "vpn-out");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn redact_masks_secrets_keeps_diagnostics() {
    let outbound = json!({
        "type": "vless",
        "server": "server.example",
        "uuid": "b831381d-secret",
        "tls": { "reality": { "public_key": "PUBKEY", "short_id": "1a2b" } }
    });
    let r = crate::redact::redact_value(&outbound);
    assert_eq!(r["uuid"], "***");
    assert_eq!(r["tls"]["reality"]["short_id"], "***");
    assert_eq!(r["type"], "vless"); // diagnostic kept
    assert_eq!(r["server"], "server.example"); // endpoint kept
    assert_eq!(r["tls"]["reality"]["public_key"], "PUBKEY"); // public by design
}

#[test]
fn fetch_times_out_on_silent_server() {
    // Bound listener that never accepts/responds: connect succeeds via the
    // kernel backlog, then the server stays silent -> global timeout fires.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let start = std::time::Instant::now();
    let res = subscription::fetch_with_timeout(
        &format!("http://{addr}/sub"),
        std::time::Duration::from_secs(1),
    );
    assert!(res.is_err(), "silent server must yield an error, got Ok");
    assert!(
        start.elapsed() < std::time::Duration::from_secs(5),
        "timed out too slowly: {:?}",
        start.elapsed()
    );
    drop(listener);
}

// ---------- proxy mode ----------

const PROXY_SUB: &str = include_str!("../tests/fixtures/proxy-sub.txt");
const PROXY_EXAMPLE: &str = include_str!("../examples/gateway-proxy.toml");
const GOLDEN_PROXY_URLTEST: &str = include_str!("../tests/golden/proxy-urltest.sing-box.json");
const GOLDEN_PROXY_PINNED: &str = include_str!("../tests/golden/proxy-pinned.sing-box.json");
const GOLDEN_GATEWAY_MINIMAL: &str = include_str!("../tests/golden/gateway-minimal.sing-box.json");

const PROXY_URLTEST_TOML: &str = r#"
mode = "proxy"
[proxy]
port = 12080
[subscription]
url = "https://example.com/sub"
strategy = "urltest"
"#;

const PROXY_PINNED_TOML: &str = r#"
mode = "proxy"
[proxy]
listen = "127.0.0.1"
port = 12080
[subscription]
url = "https://example.com/sub"
strategy = "pinned"
active = "Node B"
"#;

// Gateway config OMITTING tun/dns/killswitch/management/subscription — locks the
// None->default render so an omitted section can never silently shift bytes.
const GATEWAY_MINIMAL_TOML: &str = r#"
[interfaces]
wan = "eth0"
lan = "br0"
lan_cidr = "192.168.10.0/24"
[routing]
mode = "full"
[[policies]]
name = "all"
source = "192.168.10.0/24"
route = "vpn"
"#;

fn proxy_cfg(s: &str) -> config::GatewayConfig {
    toml::from_str(s).expect("proxy config parses")
}

fn fixture_outbounds() -> Vec<serde_json::Value> {
    subscription::parse_subscription(PROXY_SUB)
        .unwrap()
        .outbounds
        .iter()
        .map(|o| o.outbound.clone())
        .collect()
}

fn err_codes(s: &str) -> Vec<&'static str> {
    let cfg: config::GatewayConfig = toml::from_str(s).expect("test config parses");
    config::validate(&cfg).0.iter().map(|f| f.code).collect()
}

#[test]
fn proxy_example_validates_and_schema_has_proxy() {
    let cfg: config::GatewayConfig = toml::from_str(&norm(PROXY_EXAMPLE)).unwrap();
    assert_eq!(cfg.mode, config::Mode::Proxy);
    assert!(
        config::validate(&cfg).0.is_empty(),
        "proxy example must validate clean"
    );
    let schema: serde_json::Value = serde_json::from_str(crate::SCHEMA).unwrap();
    assert!(schema["properties"]["mode"].is_object());
    assert!(schema["properties"]["proxy"].is_object());
}

#[test]
fn proxy_config_parses_and_defaults() {
    let cfg = proxy_cfg(PROXY_URLTEST_TOML);
    assert_eq!(cfg.mode, config::Mode::Proxy);
    let p = cfg.proxy.as_ref().unwrap();
    assert_eq!(p.listen, "::");
    assert_eq!(p.port, 12080);
    assert_eq!(
        cfg.subscription.as_ref().unwrap().strategy,
        config::Strategy::Urltest
    );
    assert!(config::validate(&cfg).0.is_empty());
}

#[test]
fn gateway_minimal_validates_with_defaults() {
    let cfg = proxy_cfg(GATEWAY_MINIMAL_TOML);
    assert_eq!(cfg.mode, config::Mode::Gateway);
    assert_eq!(cfg.tun_mtu(), 1420);
    assert_eq!(cfg.dns_mode(), config::DnsMode::Tunneled);
    assert!(!cfg.killswitch_enabled());
    assert!(config::validate(&cfg).0.is_empty());
}

#[test]
fn proxy_missing_port_is_parse_error() {
    let s = "mode=\"proxy\"\n[proxy]\nlisten=\"::\"\n[subscription]\nurl=\"https://x/s\"\nstrategy=\"urltest\"\n";
    assert!(toml::from_str::<config::GatewayConfig>(s).is_err());
}

#[test]
fn proxy_validation_matrix() {
    assert!(err_codes(
        "mode=\"proxy\"\n[subscription]\nurl=\"https://x/s\"\nstrategy=\"urltest\"\n"
    )
    .contains(&"PROXY_SECTION_REQUIRED"));
    let conflict = err_codes("mode=\"proxy\"\n[proxy]\nport=12080\n[subscription]\nurl=\"https://x/s\"\nstrategy=\"urltest\"\n[tun]\nmtu=1400\n");
    assert!(conflict.contains(&"PROXY_MODE_SECTION_CONFLICT"));
    assert!(
        err_codes("mode=\"proxy\"\n[proxy]\nport=12080\n").contains(&"PROXY_SUBSCRIPTION_REQUIRED")
    );
    assert!(err_codes("mode=\"proxy\"\n[proxy]\nport=0\n[subscription]\nurl=\"https://x/s\"\nstrategy=\"urltest\"\n")
        .contains(&"PORT_INVALID"));
    assert!(err_codes("mode=\"proxy\"\n[proxy]\nport=12080\n[subscription]\nurl=\"https://x/s\"\nstrategy=\"pinned\"\n")
        .contains(&"SUBSCRIPTION_ACTIVE_EMPTY"));
    assert!(err_codes("mode=\"proxy\"\n[proxy]\nport=12080\n[subscription]\nurl=\"https://x/s\"\nstrategy=\"urltest\"\nactive=\"X\"\n")
        .contains(&"URLTEST_ACTIVE_CONFLICT"));
    assert!(err_codes("[interfaces]\nwan=\"e\"\nlan=\"b\"\nlan_cidr=\"10.0.0.0/24\"\n[routing]\nmode=\"full\"\n[[policies]]\nname=\"a\"\nsource=\"10.0.0.0/24\"\nroute=\"vpn\"\n[subscription]\nurl=\"https://x/s\"\nstrategy=\"urltest\"\n")
        .contains(&"URLTEST_GATEWAY_UNSUPPORTED"));
    assert!(err_codes("[routing]\nmode=\"full\"\n[[policies]]\nname=\"a\"\nsource=\"10.0.0.0/24\"\nroute=\"direct\"\n")
        .contains(&"GATEWAY_INTERFACES_REQUIRED"));
    assert!(err_codes("mode=\"proxy\"\n[proxy]\nport=12080\n[subscription]\nurl=\"ftp://x\"\nstrategy=\"urltest\"\n")
        .contains(&"SUBSCRIPTION_URL_INVALID"));
}

#[test]
fn node_safety_skips_plaintext_and_reality_without_pbk() {
    let parsed = subscription::parse_subscription(PROXY_SUB).unwrap();
    assert_eq!(parsed.outbounds.len(), 3, "Node A x2 + Node B usable");
    assert!(parsed
        .skipped
        .iter()
        .any(|s| s.name == "Node C" && s.reason.contains("plaintext")));
    // reality without pbk, as the only node -> all-unsafe error
    let s = "vless://55555555-5555-5555-5555-555555555555@e.example.com:443?security=reality&sni=x&type=tcp#Bad\n";
    assert!(subscription::parse_subscription(s).is_err());
}

#[test]
fn parse_hysteria2_shape() {
    // trailing / in host:port + insecure=1, as real ninitux nodes emit.
    let uri = "hysteria2://mypassword@h.example.com:8444/?sni=h.example.com&insecure=1&obfs=salamander&obfs-password=obfspw&pair=cdn#Latvia%20HY2";
    let parsed = subscription::parse_subscription(uri).unwrap();
    assert_eq!(parsed.outbounds.len(), 1);
    assert_eq!(parsed.outbounds[0].name, "Latvia HY2");
    let o = &parsed.outbounds[0].outbound;
    assert_eq!(o["type"], "hysteria2");
    assert_eq!(o["server"], "h.example.com");
    assert_eq!(o["server_port"], 8444);
    assert_eq!(o["password"], "mypassword");
    assert_eq!(o["tls"]["server_name"], "h.example.com");
    assert_eq!(o["tls"]["insecure"], true);
    assert_eq!(o["tls"]["alpn"][0], "h3");
    assert_eq!(o["obfs"]["type"], "salamander");
    assert_eq!(o["obfs"]["password"], "obfspw");
}

#[test]
fn redact_masks_hysteria2_password_and_obfs() {
    let uri = "hysteria2://secretpw@h:8444/?obfs=salamander&obfs-password=obfssecret#X";
    let o = &subscription::parse_subscription(uri).unwrap().outbounds[0].outbound;
    let r = crate::redact::redact_value(o);
    assert_eq!(r["password"], "***");
    assert_eq!(r["obfs"]["password"], "***");
    assert_eq!(r["type"], "hysteria2");
    assert_eq!(r["server"], "h"); // endpoint kept
}

#[test]
fn cache_v2_roundtrip_and_v1_compat() {
    let d = tmpdir("cachev2");
    let parsed = subscription::parse_subscription(PROXY_SUB).unwrap();
    subscription::save_cache_all(&d, "https://sub.example/t", None, &parsed.outbounds).unwrap();
    let cached = subscription::load_cache(&d).unwrap().unwrap();
    assert_eq!(cached.outbounds.len(), 3);
    assert!(cached.active.is_none());
    assert!(subscription::load_resolved(&d).is_some());
    let cache_doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(d.join("subscription.json")).unwrap())
            .unwrap();
    assert_eq!(cache_doc["v"], 2);
    assert_eq!(cache_doc["source"], "https://sub.example/…");

    let d1 = tmpdir("cachev1");
    let chosen = subscription::select(&parsed.outbounds, "Node B").unwrap();
    subscription::save_cache(&d1, "https://s/t", chosen).unwrap();
    let c1 = subscription::load_cache(&d1).unwrap().unwrap();
    assert_eq!(c1.outbounds.len(), 1);
    assert_eq!(c1.active.as_deref(), Some("Node B"));
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&d1);
}

#[test]
fn cache_unknown_version_errors() {
    let d = tmpdir("cacheunk");
    std::fs::write(d.join("subscription.json"), r#"{"v":99,"outbounds":[]}"#).unwrap();
    assert!(subscription::load_cache(&d).is_err());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn proxy_render_matches_golden() {
    let cfg_u = proxy_cfg(PROXY_URLTEST_TOML);
    let obs = fixture_outbounds();
    let u = render::render_proxy_sing_box(&cfg_u, &obs);

    let cfg_p = proxy_cfg(PROXY_PINNED_TOML);
    let parsed = subscription::parse_subscription(PROXY_SUB).unwrap();
    let node_b = &subscription::select(&parsed.outbounds, "Node B")
        .unwrap()
        .outbound;
    let p = render::render_proxy_sing_box(&cfg_p, std::slice::from_ref(node_b));

    let cfg_g = proxy_cfg(GATEWAY_MINIMAL_TOML);
    let g = render::render_sing_box(&cfg_g, None);

    if std::env::var("UPDATE_GOLDEN").is_ok() {
        std::fs::write("tests/golden/proxy-urltest.sing-box.json", &u).unwrap();
        std::fs::write("tests/golden/proxy-pinned.sing-box.json", &p).unwrap();
        std::fs::write("tests/golden/gateway-minimal.sing-box.json", &g).unwrap();
    }
    assert_eq!(
        norm(&u),
        norm(GOLDEN_PROXY_URLTEST),
        "proxy urltest render drifted"
    );
    assert_eq!(
        norm(&p),
        norm(GOLDEN_PROXY_PINNED),
        "proxy pinned render drifted"
    );
    assert_eq!(
        norm(&g),
        norm(GOLDEN_GATEWAY_MINIMAL),
        "gateway-minimal render drifted"
    );

    // determinism
    assert_eq!(u, render::render_proxy_sing_box(&cfg_u, &obs));

    // dedup: duplicate "Node A" -> Node A / Node A-1, and urltest references them
    let v: serde_json::Value = serde_json::from_str(&u).unwrap();
    let tags: Vec<&str> = v["outbounds"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|o| o["type"] == "vless")
        .map(|o| o["tag"].as_str().unwrap())
        .collect();
    assert_eq!(tags, vec!["Node A", "Node A-1", "Node B"]);
    assert_eq!(v["route"]["final"], "auto");
    assert_eq!(v["inbounds"][0]["type"], "mixed");
    assert!(
        !u.contains("\"direct\""),
        "no direct outbound in proxy mode"
    );
    assert!(!v.as_object().unwrap().contains_key("dns"), "no dns block");

    let vp: serde_json::Value = serde_json::from_str(&p).unwrap();
    assert_eq!(vp["route"]["final"], "Node B");
    assert_eq!(vp["outbounds"].as_array().unwrap().len(), 1);
}

#[test]
fn proxy_plan_singbox_only_no_ssh_risk() {
    let cfg = proxy_cfg(PROXY_URLTEST_TOML);
    let d = tmpdir("proxyplan");
    let out = plan::build_plan(&cfg, &[], Path::new("x"), &d, Some("192.168.10.77 5 1 22"));
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let changes = v["changes"].as_array().unwrap();
    assert_eq!(changes.len(), 1, "proxy renders sing-box only, no nft");
    assert_eq!(changes[0]["target"], "sing-box");
    let risks = v["risks"].as_array().unwrap();
    assert!(risks.iter().any(|r| r["code"] == "OUTBOUND_UNRESOLVED"));
    assert!(!risks.iter().any(|r| r["code"] == "SSH_MAY_DROP"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn proxy_mode_refuses_host_commands() {
    let err = crate::refuse_if_proxy(&proxy_cfg(PROXY_URLTEST_TOML)).unwrap_err();
    assert_eq!(err.code, "PROXY_MODE_NOT_APPLYABLE");
    assert_eq!(err.exit, 2);
    assert!(crate::refuse_if_proxy(&sample()).is_ok());
}

#[test]
fn render_command_envelope_is_content_free() {
    let d = tmpdir("rendercmd");
    let cfgfile = d.join("proxy.toml");
    std::fs::write(&cfgfile, PROXY_URLTEST_TOML).unwrap();
    subscription::save_cache_all(
        &d,
        "https://x/s",
        None,
        &subscription::parse_subscription(PROXY_SUB)
            .unwrap()
            .outbounds,
    )
    .unwrap();
    let out = crate::cmd_render(&cfgfile, &d, Some(d.join("out"))).unwrap();
    assert!(
        !out.contains("11111111-1111"),
        "render envelope leaked a uuid"
    );
    assert!(out.contains("\"bytes\""));
    // the real uuid IS on disk (root-only artifact), just never in the envelope
    let rendered = std::fs::read_to_string(d.join("out").join("sing-box.json")).unwrap();
    assert!(rendered.contains("11111111-1111"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn redact_url_keeps_host_drops_token() {
    assert_eq!(
        crate::redact::redact_url("https://sub.example/api?token=SECRET"),
        "https://sub.example/…"
    );
    assert_eq!(
        crate::redact::redact_url("https://user:pass@host.example/x"),
        "https://host.example/…"
    );
    assert_eq!(crate::redact::redact_url("garbage-no-scheme"), "***");
}
