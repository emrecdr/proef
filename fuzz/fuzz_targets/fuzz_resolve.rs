//! Resolver totality: arbitrary template text must never panic and always
//! terminate under the depth cap (TESTING-STRATEGY).
#![no_main]

use std::collections::BTreeMap;

use libfuzzer_sys::fuzz_target;
use proef_core::resolve::{self, ResolveCtx, ResolveMode};
use proef_core::world::World;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        let world = World::default();
        let empty = BTreeMap::new();
        // Self-referential scope: worst case for the recursion cap.
        let args: BTreeMap<String, String> =
            [("a".to_owned(), "${a}".to_owned())].into_iter().collect();
        let ctx = ResolveCtx {
            args: &args,
            defaults: &empty,
            env: &empty,
            config_vars: &empty,
            run_id: "fuzz-run",
            world: &world,
            mode: ResolveMode::DryRun,
        };
        let _ = resolve::resolve(text, &ctx);
    }
});
