//! Thin shim for the feature-gated Phase 4 product-path evaluation driver.
//! All logic lives in the desktop lib behind `product-eval`; this binary only
//! forwards argv and the exit code.

fn main() {
    let code = cindx_desktop::product_eval_main(std::env::args().skip(1).collect());
    std::process::exit(code);
}
