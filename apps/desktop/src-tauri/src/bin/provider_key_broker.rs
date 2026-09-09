//! Thin shim for the feature-gated provider-key broker. All logic lives in
//! the desktop lib behind `product-eval`; this binary only forwards the exit
//! code. It is signed with the desktop app's designated requirement so the
//! keychain item the app created answers silently — see
//! `provider_key_broker_main` for the full rationale.

fn main() {
    let code = cindx_desktop::provider_key_broker_main();
    std::process::exit(code);
}
