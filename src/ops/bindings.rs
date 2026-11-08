//! The TypeScript side of the Operations interface: gui/src/lib/bindings.ts holds the
//! declaration of every type the GUI sends or receives, generated from the Rust types (ts-rs).
//! The test fails if the file is stale; `UPDATE_BINDINGS=1 cargo test bindings` rewrites it.

use super::query;
use std::any::TypeId;
use std::collections::{BTreeMap, HashSet};
use ts_rs::{Config, TS, TypeVisitor};

const FILE: &str = "gui/src/lib/bindings.ts";

/// The types at the seam; everything they reference is declared too.
fn roots(c: &mut Collect) {
    c.visit::<query::Query>();
    c.visit::<super::Action>();
    c.visit::<super::render::LogLine>();
    c.visit::<super::render::ActResult>();
    c.visit::<query::AppState>();
    c.visit::<query::ModInfo>();
    c.visit::<query::Conflicts>();
    c.visit::<query::UpdateRow>();
    c.visit::<query::OverwriteRow>();
    c.visit::<query::TrackedRow>();
    c.visit::<query::Lookup>();
    c.visit::<query::LoadOrder>();
    c.visit::<crate::health::Finding>();
    c.visit::<crate::loadorder::Pair>();
    c.visit::<query::SafetyView>();
    c.visit::<crate::safety::HistEntry>();
    c.visit::<query::SettingsView>();
    c.visit::<crate::downloads::Download>();
    c.visit::<query::Onboarding>();
    c.visit::<query::ChangelogView>();
    c.visit::<query::ModioView>();
    c.visit::<query::FomodView>();
    c.visit::<query::CollectionView>();
    c.visit::<query::ToolsView>();
    c.visit::<query::ToolFile>();
}

/// Declarations by TypeScript name (sorted, so the file is stable).
struct Collect {
    cfg: Config,
    seen: HashSet<TypeId>,
    decls: BTreeMap<String, String>,
}

impl TypeVisitor for Collect {
    fn visit<T: TS + 'static + ?Sized>(&mut self) {
        if T::output_path().is_none() || !self.seen.insert(TypeId::of::<T>()) {
            return;
        }
        let name = T::ident(&self.cfg);
        let decl = format!("{}export {}", T::docs().unwrap_or_default(), T::decl(&self.cfg));
        let prev = self.decls.insert(name.clone(), decl);
        assert!(prev.is_none(), "two Rust types are both {name} in TypeScript");
        T::visit_dependencies(self);
    }
}

fn typescript() -> String {
    // numbers in JSON are numbers in TypeScript, whatever their Rust width
    let mut c = Collect { cfg: Config::new().with_large_int("number"), seen: HashSet::new(), decls: BTreeMap::new() };
    roots(&mut c);
    let mut out = String::from(
        "// Generated from the Rust types of the Operations interface (src/ops/bindings.rs). Do not edit:\n\
         // `UPDATE_BINDINGS=1 cargo test bindings` regenerates it.\n",
    );
    for d in c.decls.values() {
        out.push('\n');
        out.push_str(d);
        out.push('\n');
    }
    out
}

#[test]
fn bindings_are_current() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(FILE);
    let want = typescript();
    if std::env::var_os("UPDATE_BINDINGS").is_some() {
        std::fs::write(&path, &want).unwrap();
    }
    let have = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(have == want, "{FILE} is stale: run `UPDATE_BINDINGS=1 cargo test bindings` and commit it");
}
