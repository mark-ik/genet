/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use super::*;

fn request(address: &str) -> EngineRouteRequest {
    EngineRouteRequest {
        workspace_id: WorkspaceRouteId::new("main"),
        view: None,
        node: None,
        address: address.to_string(),
        content_type: None,
        pinned_engine: None,
    }
}

fn request_with_content_type(address: &str, content_type: &str) -> EngineRouteRequest {
    EngineRouteRequest {
        workspace_id: WorkspaceRouteId::new("main"),
        view: None,
        node: None,
        address: address.to_string(),
        content_type: Some(content_type.to_string()),
        pinned_engine: None,
    }
}

#[test]
fn default_policy_routes_full_web_to_genet() {
    let decision = EngineRoutePolicy::default().route(&request("https://example.test"));

    assert_eq!(decision.engine_id, ENGINE_GENET_WEB);
    assert_eq!(
        decision.surface_contract.mode,
        SurfaceContractMode::CompositedTexture
    );
}

#[test]
fn default_policy_routes_gemini_to_gemtext() {
    let decision = EngineRoutePolicy::default().route(&request("gemini://example.test"));

    assert_eq!(decision.engine_id, ENGINE_NEMATIC_GEMTEXT);
}

#[test]
fn default_policy_routes_gopher_to_gopher_engine() {
    let decision = EngineRoutePolicy::default().route(&request("gopher://example.test"));

    assert_eq!(decision.engine_id, ENGINE_NEMATIC_GOPHER);
}

#[test]
fn default_policy_routes_finger_to_finger_engine() {
    let decision = EngineRoutePolicy::default().route(&request("finger://user@example.test"));

    assert_eq!(decision.engine_id, ENGINE_NEMATIC_FINGER);
}

// (Internal-page routing — mere's `graphshell://` and friends — is the host's
// own rule, layered onto the policy app-side; its test lives with
// `mere::routing`.)

#[test]
fn default_policy_does_not_guess_unknown_protocols() {
    let decision = EngineRoutePolicy::default().route(&request("mailto:hello@example.test"));

    assert_eq!(decision.engine_id, ENGINE_EXTERNAL_PROTOCOL);
    assert_eq!(
        decision.surface_contract.mode,
        SurfaceContractMode::Headless
    );
}

#[test]
fn route_target_prefers_node_identity() {
    let mut request = request("https://example.test");
    request.node = Some(NodeKey::new(42));

    let decision = EngineRoutePolicy::default().route(&request);

    assert_eq!(decision.surface_contract.target.as_str(), "node:42");
}

#[test]
fn content_type_routes_markdown_regardless_of_scheme() {
    let policy = EngineRoutePolicy::default();

    let from_https = policy.route(&request_with_content_type(
        "https://example.test/post.md",
        "text/markdown",
    ));
    let from_file = policy.route(&request_with_content_type(
        "file:///home/user/notes.md",
        "text/markdown",
    ));

    assert_eq!(from_https.engine_id, ENGINE_NEMATIC_MARKDOWN);
    assert_eq!(from_file.engine_id, ENGINE_NEMATIC_MARKDOWN);
}

#[test]
fn content_type_wins_over_scheme_match() {
    // https alone would route to Genet; with text/plain it should
    // route to the text engine instead.
    let decision = EngineRoutePolicy::default().route(&request_with_content_type(
        "https://example.test/raw.txt",
        "text/plain",
    ));
    assert_eq!(decision.engine_id, ENGINE_NEMATIC_TEXT);
}

#[test]
fn content_type_with_parameters_still_matches() {
    let decision = EngineRoutePolicy::default().route(&request_with_content_type(
        "https://example.test/",
        "text/markdown; charset=utf-8",
    ));
    assert_eq!(decision.engine_id, ENGINE_NEMATIC_MARKDOWN);
}

#[test]
fn unknown_content_type_falls_back_to_scheme() {
    let decision = EngineRoutePolicy::default().route(&request_with_content_type(
        "https://example.test/",
        "application/x-unknown-format",
    ));
    // Unknown content-type → no content-type rule matches → fall back
    // to the scheme rule (https → Genet).
    assert_eq!(decision.engine_id, ENGINE_GENET_WEB);
}

// (JSON-LD graph-contribution routing is a host-handled marker, layered onto
// the policy app-side; its test lives with `mere::routing`.)

#[test]
fn content_type_match_is_case_insensitive() {
    let decision = EngineRoutePolicy::default().route(&request_with_content_type(
        "https://example.test/",
        "TEXT/Gemini",
    ));
    assert_eq!(decision.engine_id, ENGINE_NEMATIC_GEMTEXT);
}

#[test]
fn route_filtered_skips_rules_whose_engine_is_unavailable() {
    // Only the gemtext engine is "registered" here. https would normally
    // route to Genet; with that filtered out, routing falls through to
    // the fallback (no other scheme rule matches https).
    let policy = EngineRoutePolicy::default();
    let decision = policy.route_filtered(&request("https://example.test"), |id| {
        id == ENGINE_NEMATIC_GEMTEXT
    });
    assert_eq!(decision.engine_id, ENGINE_EXTERNAL_PROTOCOL);
}

#[test]
fn route_filtered_uses_first_available_matching_rule() {
    let policy = EngineRoutePolicy::default();
    // Both gemini scheme and text/gemini content-type route to gemtext.
    // Filter the gemtext engine away and the gemini scheme has no other
    // rule, so routing falls through to the external-protocol fallback.
    let decision = policy.route_filtered(&request("gemini://example.test"), |id| {
        id != ENGINE_NEMATIC_GEMTEXT
    });
    assert_eq!(decision.engine_id, ENGINE_EXTERNAL_PROTOCOL);
}

#[test]
fn route_filtered_lets_content_type_rule_fall_back_to_scheme() {
    // text/markdown normally wins over scheme. With the markdown engine
    // filtered out, the scheme rule (https → Genet) takes over.
    let policy = EngineRoutePolicy::default();
    let decision = policy.route_filtered(
        &request_with_content_type("https://example.test/", "text/markdown"),
        |id| id != ENGINE_NEMATIC_MARKDOWN,
    );
    assert_eq!(decision.engine_id, ENGINE_GENET_WEB);
}

#[test]
fn host_from_address_strips_scheme_userinfo_and_port() {
    assert_eq!(
        host_from_address("https://example.com/path"),
        Some("example.com")
    );
    assert_eq!(
        host_from_address("https://user:pass@example.com:8080/x"),
        Some("example.com")
    );
    assert_eq!(
        host_from_address("gemini://capsule.test/"),
        Some("capsule.test")
    );
    assert_eq!(host_from_address("mailto:foo@example.com"), None);
    assert_eq!(host_from_address("about:blank"), None);
}

#[test]
fn pinned_engine_wins_over_scheme_and_content_type() {
    let policy = EngineRoutePolicy::default();
    let mut req = request_with_content_type("https://example.test/", "text/markdown");
    req.pinned_engine = Some(ENGINE_NEMATIC_TEXT.to_string());

    let decision = policy.route(&req);
    assert_eq!(decision.engine_id, ENGINE_NEMATIC_TEXT);
}

#[test]
fn pinned_engine_skipped_when_not_available() {
    let policy = EngineRoutePolicy::default();
    let mut req = request("https://example.test/");
    req.pinned_engine = Some("not.registered".to_string());

    // Filter says nothing called "not.registered" exists; routing falls
    // through to the scheme rule.
    let decision = policy.route_filtered(&req, |id| id != "not.registered");
    assert_eq!(decision.engine_id, ENGINE_GENET_WEB);
}

#[test]
fn scrying_web_pin_wins_over_default_genet_routing() {
    // `scrying.web` isn't in the default policy — it's opt-in per
    // tile. A user pinning it should route through.
    let policy = EngineRoutePolicy::default();
    let mut req = request("https://example.test/");
    req.pinned_engine = Some(ENGINE_SCRYING_WEB.to_string());
    let decision = policy.route(&req);
    assert_eq!(decision.engine_id, ENGINE_SCRYING_WEB);
}

#[test]
fn per_host_override_wins_over_scheme_rule() {
    let mut policy = EngineRoutePolicy::default();
    policy.per_host_overrides.insert(
        "docs.example.com".to_string(),
        ENGINE_NEMATIC_MARKDOWN.to_string(),
    );

    let decision = policy.route(&request("https://docs.example.com/page"));
    assert_eq!(decision.engine_id, ENGINE_NEMATIC_MARKDOWN);

    // Other hosts on the same scheme still go through the scheme rule.
    let other = policy.route(&request("https://example.test/"));
    assert_eq!(other.engine_id, ENGINE_GENET_WEB);
}

#[test]
fn per_host_override_loses_to_content_type_rule() {
    // Content-type is server's claim; per-host is the user's general
    // pref. Server wins.
    let mut policy = EngineRoutePolicy::default();
    policy
        .per_host_overrides
        .insert("blog.test".to_string(), ENGINE_NEMATIC_TEXT.to_string());

    let decision = policy.route(&request_with_content_type(
        "https://blog.test/post",
        "text/markdown",
    ));
    assert_eq!(decision.engine_id, ENGINE_NEMATIC_MARKDOWN);
}

#[test]
fn per_host_override_skipped_when_engine_not_available() {
    let mut policy = EngineRoutePolicy::default();
    policy
        .per_host_overrides
        .insert("blog.test".to_string(), "not.registered".to_string());

    let decision =
        policy.route_filtered(&request("https://blog.test/"), |id| id != "not.registered");
    // Falls through to the scheme rule.
    assert_eq!(decision.engine_id, ENGINE_GENET_WEB);
}

#[test]
fn per_host_override_match_is_case_insensitive() {
    let mut policy = EngineRoutePolicy::default();
    policy
        .per_host_overrides
        .insert("Blog.Test".to_string(), ENGINE_NEMATIC_TEXT.to_string());

    let decision = policy.route(&request("https://blog.test/post"));
    assert_eq!(decision.engine_id, ENGINE_NEMATIC_TEXT);
}

#[test]
fn genet_rungs_classify_and_round_trip() {
    // genet.web is the static rung (the legacy id, kept for pin compatibility).
    assert_eq!(genet_rung(ENGINE_GENET_WEB), Some(GenetRung::Static));
    assert_eq!(
        genet_rung(ENGINE_GENET_SCRIPTED),
        Some(GenetRung::Scripted)
    );
    assert_eq!(
        genet_rung(ENGINE_GENET_SCRIPTED_NOVA),
        Some(GenetRung::Scripted)
    );
    // Each rung's engine id round-trips back to the rung.
    for rung in GenetRung::ALL {
        assert_eq!(genet_rung(rung.engine_id()), Some(rung));
    }
    // Non-genet engines are not rungs.
    assert_eq!(genet_rung(ENGINE_SCRYING_WEB), None);
    assert_eq!(genet_rung(ENGINE_NEMATIC_GEMTEXT), None);
    assert!(is_genet_rung(ENGINE_GENET_WEB) && !is_genet_rung(ENGINE_SCRYING_WEB));
}

#[test]
fn genet_rungs_order_by_capability() {
    assert!(GenetRung::Static < GenetRung::Interactive);
    assert!(GenetRung::Interactive < GenetRung::Scripted);
    assert!(GenetRung::Scripted < GenetRung::FullWeb);
    // Static is the base of the ladder (the default, JS-free rung).
    assert_eq!(GenetRung::ALL[0], GenetRung::Static);
}

#[test]
fn unregistered_higher_rung_pin_falls_back_to_static() {
    // A node pinned to a rung the host has not registered (e.g. scripted before it
    // ships) must not route to an unavailable engine: `route_filtered` walks past the
    // pin to the scheme rule, which for an https page is the static genet rung.
    let mut req = request("https://example.com/app");
    req.pinned_engine = Some(ENGINE_GENET_SCRIPTED.to_string());
    // Only the static rung is "registered" on this host.
    let decision = EngineRoutePolicy::default().route_filtered(&req, |id| id == ENGINE_GENET_WEB);
    assert_eq!(decision.engine_id, ENGINE_GENET_WEB);
}
