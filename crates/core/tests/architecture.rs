//! The direction of dependencies between the top-level modules of
//! `socium-core`. A module may use only modules in lower layers, so the
//! graph has no cycles and a change low in the stack cannot need one high
//! up. Test code is not checked: it may reach anything to set up a case.
//!
//! See docs/architecture.md for what each layer is for.
use proc_macro2::Span;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use syn::visit::Visit;

/// Lowest layer first. A module may use the modules of the layers before
/// its own, never those of its own layer or later ones. `lib.rs` itself is
/// the facade above all of them.
const LAYERS: &[&[&str]] = &[
    &["constants", "host", "tool_error"],
    &["conversation", "models"],
    &["state", "providers"],
    &["plan", "settings", "error"],
    &["activity", "execution", "workspace"],
    &["transcript", "usage", "access", "activity_log"],
    &["progress"],
    &["control"],
    &["process", "mcp"],
    &["tools"],
    &["approval", "instructions"],
    &["dispatch", "session"],
    &["agent", "model_import"],
];

/// A use of another module, as `file:line` and the path that was written.
struct Use {
    from: String,
    to: String,
    path: String,
    at: String,
}

#[test]
fn modules_use_only_lower_layers() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let (unknown, violations) = check(&src, LAYERS);
    assert!(
        unknown.is_empty(),
        "These top-level modules are not in LAYERS in crates/core/tests/architecture.rs: {unknown:?}.\n\
         Put each in the lowest layer that holds everything it uses, and describe it in docs/architecture.md."
    );
    assert!(
        violations.is_empty(),
        "{} uses point up or sideways in the layers:\n  {}\n\n\
         A module may use only modules in lower layers (LAYERS in crates/core/tests/architecture.rs).\n\
         Fix the design, not the check: move what both modules need down into a lower module, \
         or let the caller pass it in (a value, a function, or a trait the lower module defines).\n\
         Do not re-export the item from another module to hide the use. Move a module up a layer \
         only when nothing below uses it, and say why in docs/architecture.md.",
        violations.len(),
        violations.join("\n  ")
    );
}

#[test]
fn the_check_reports_a_use_of_a_higher_layer_and_ignores_tests() {
    let src = tempfile::tempdir().unwrap();
    let write = |name: &str, text: &str| std::fs::write(src.path().join(name), text).unwrap();
    write(
        "lib.rs",
        "mod high;\nmod low;\npub use high::Thing;\npub struct Facade;\n",
    );
    write(
        "high.rs",
        "pub struct Thing;\nfn f() -> crate::low::Low { crate::low::Low }\n",
    );
    write(
        "low.rs",
        "pub struct Low;\n\
         fn up() -> crate::Thing { crate::high::Thing }\n\
         fn facade(_: &crate::Facade) {}\n\
         #[cfg(test)]\nmod tests { use crate::high::Thing; }\n",
    );
    let (unknown, violations) = check(src.path(), &[&["low"], &["high"]]);

    assert!(unknown.is_empty(), "{unknown:?}");
    let lines: BTreeSet<_> = violations
        .iter()
        .map(|v| v.split(": ").nth(1).unwrap().to_string())
        .collect();
    assert_eq!(
        lines,
        BTreeSet::from([
            "`low` uses `crate::Thing`, which belongs to `high`".to_string(),
            "`low` uses `crate::high::Thing`, which belongs to `high`".to_string(),
            "`low` uses `crate::Facade`, which belongs to `crate`".to_string(),
        ]),
        "{violations:#?}"
    );
}

/// Modules missing from `layers`, and each use that points up or sideways.
fn check(src: &Path, layers: &[&[&str]]) -> (BTreeSet<String>, Vec<String>) {
    let rank = layer_ranks(layers);
    let crate_root = parse(&src.join("lib.rs"));
    let reexports = root_reexports(&crate_root);
    let modules: BTreeSet<String> = crate_root
        .items
        .iter()
        .filter_map(|item| match item {
            syn::Item::Mod(module) if !is_test_only(&module.attrs) => {
                Some(module.ident.to_string())
            }
            _ => None,
        })
        .collect();
    let mut uses = Vec::new();
    collect_module(src, &src.join("lib.rs"), &[], &crate_root.items, &mut uses);

    let unknown: BTreeSet<String> = modules
        .iter()
        .filter(|module| !rank.contains_key(module.as_str()))
        .cloned()
        .collect();
    let mut violations = Vec::new();
    for found in uses {
        // `crate::TaskControl` is `control`'s; `crate::AgentProgress`, defined
        // in lib.rs, is the facade's.
        let to = match reexports.get(&found.to) {
            Some(module) => module.clone(),
            None if modules.contains(&found.to) => found.to.clone(),
            None => ROOT.to_string(),
        };
        if to == found.from || found.from == ROOT {
            continue;
        }
        let allowed = match (rank.get(found.from.as_str()), rank.get(to.as_str())) {
            (Some(from), Some(to)) => to < from,
            _ => false,
        };
        if !allowed && !unknown.contains(&found.from) && !unknown.contains(&to) {
            violations.push(format!(
                "{at}: `{from}` uses `{path}`, which belongs to `{to}`",
                at = found.at,
                from = found.from,
                path = found.path,
            ));
        }
    }
    (unknown, violations)
}

const ROOT: &str = "crate";

fn layer_ranks<'a>(layers: &[&[&'a str]]) -> BTreeMap<&'a str, usize> {
    let mut rank = BTreeMap::new();
    for (index, layer) in layers.iter().enumerate() {
        for module in *layer {
            assert!(
                rank.insert(*module, index).is_none(),
                "{module} is in LAYERS twice"
            );
        }
    }
    rank
}

fn parse(file: &Path) -> syn::File {
    let text = std::fs::read_to_string(file).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
    syn::parse_file(&text).unwrap_or_else(|e| panic!("{}: {e}", file.display()))
}

/// Names that `lib.rs` re-exports from a module, such as `TaskControl` from
/// `control`, so a use of `crate::TaskControl` counts against `control`.
fn root_reexports(root: &syn::File) -> BTreeMap<String, String> {
    let mut names = BTreeMap::new();
    for item in &root.items {
        if let syn::Item::Use(item) = item {
            for (path, alias) in flatten(&item.tree, Vec::new()) {
                let path: Vec<_> = path
                    .into_iter()
                    .skip_while(|segment| segment == "crate")
                    .collect();
                if let (Some(first), Some(alias)) = (path.first(), alias) {
                    names.insert(alias, first.clone());
                }
            }
        }
    }
    names
}

/// Every `use` path in a tree, with the name it is bound to.
fn flatten(tree: &syn::UseTree, prefix: Vec<String>) -> Vec<(Vec<String>, Option<String>)> {
    let with = |segment: String| {
        let mut path = prefix.clone();
        path.push(segment);
        path
    };
    match tree {
        syn::UseTree::Path(path) => flatten(&path.tree, with(path.ident.to_string())),
        syn::UseTree::Name(name) => {
            let ident = name.ident.to_string();
            let bound = (ident != "self").then(|| ident.clone());
            let path = if ident == "self" { prefix } else { with(ident) };
            vec![(path, bound)]
        }
        syn::UseTree::Rename(rename) => vec![(
            with(rename.ident.to_string()),
            Some(rename.rename.to_string()),
        )],
        syn::UseTree::Glob(_) => vec![(prefix, None)],
        syn::UseTree::Group(group) => group
            .items
            .iter()
            .flat_map(|tree| flatten(tree, prefix.clone()))
            .collect(),
    }
}

/// Whether an item is compiled only for tests: `#[cfg(test)]`, or a `cfg`
/// whose condition requires `test`, such as `cfg(all(test, unix))`.
fn is_test_only(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        attr.path().is_ident("cfg")
            && attr
                .meta
                .require_list()
                .is_ok_and(|list| requires_test(&list.tokens.to_string()))
    })
}

fn requires_test(condition: &str) -> bool {
    let condition = condition.replace(' ', "");
    condition == "test"
        || condition.starts_with("all(")
            && condition.split([',', '(', ')']).any(|part| part == "test")
}

fn item_attrs(item: &syn::Item) -> &[syn::Attribute] {
    match item {
        syn::Item::Const(i) => &i.attrs,
        syn::Item::Enum(i) => &i.attrs,
        syn::Item::Fn(i) => &i.attrs,
        syn::Item::Impl(i) => &i.attrs,
        syn::Item::Macro(i) => &i.attrs,
        syn::Item::Mod(i) => &i.attrs,
        syn::Item::Static(i) => &i.attrs,
        syn::Item::Struct(i) => &i.attrs,
        syn::Item::Trait(i) => &i.attrs,
        syn::Item::Type(i) => &i.attrs,
        syn::Item::Use(i) => &i.attrs,
        _ => &[],
    }
}

/// Collects the uses in one module's items and follows its submodules.
fn collect_module(
    src: &Path,
    file: &Path,
    module: &[String],
    items: &[syn::Item],
    uses: &mut Vec<Use>,
) {
    let mut visitor = Collector { file, module, uses };
    for item in items {
        if is_test_only(item_attrs(item)) {
            continue;
        }
        match item {
            syn::Item::Mod(child) => {
                let mut path = module.to_vec();
                path.push(child.ident.to_string());
                match &child.content {
                    Some((_, inner)) => collect_module(src, file, &path, inner, visitor.uses),
                    None => {
                        let child_file = module_file(src, &path);
                        let parsed = parse(&child_file);
                        collect_module(src, &child_file, &path, &parsed.items, visitor.uses);
                    }
                }
            }
            other => visitor.visit_item(other),
        }
    }
}

/// `a/b.rs` or `a/b/mod.rs` for the module `a::b`.
fn module_file(src: &Path, module: &[String]) -> PathBuf {
    let base = module
        .iter()
        .fold(src.to_path_buf(), |dir, part| dir.join(part));
    let flat = base.with_extension("rs");
    if flat.exists() {
        flat
    } else {
        base.join("mod.rs")
    }
}

struct Collector<'a> {
    file: &'a Path,
    module: &'a [String],
    uses: &'a mut Vec<Use>,
}

impl Collector<'_> {
    fn record(&mut self, segments: &[String], span: Span) {
        let Some(absolute) = self.absolute(segments) else {
            return;
        };
        let from = self.module.first().cloned().unwrap_or(ROOT.into());
        let to = absolute.first().cloned().unwrap_or(ROOT.into());
        let repository = Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2);
        let file = repository
            .and_then(|root| self.file.strip_prefix(root).ok())
            .unwrap_or(self.file);
        self.uses.push(Use {
            from,
            to,
            path: format!("crate::{}", absolute.join("::")),
            at: format!("{}:{}", file.display(), span.start().line),
        });
    }

    /// The path from the crate root, for `crate::`, `super::` and `self::`
    /// paths; `None` for other crates and names in scope.
    fn absolute(&self, segments: &[String]) -> Option<Vec<String>> {
        // A lone `self` is the receiver in an expression, not a module.
        if segments.len() == 1 {
            return None;
        }
        let mut base = match segments.first()?.as_str() {
            "crate" => Vec::new(),
            "self" | "super" => self.module.to_vec(),
            _ => return None,
        };
        let mut rest = segments;
        if rest[0] == "crate" || rest[0] == "self" {
            rest = &rest[1..];
        }
        while rest.first().is_some_and(|segment| segment == "super") {
            base.pop();
            rest = &rest[1..];
        }
        base.extend(rest.iter().cloned());
        Some(base)
    }
}

impl<'ast> Visit<'ast> for Collector<'_> {
    fn visit_item_use(&mut self, item: &'ast syn::ItemUse) {
        if is_test_only(&item.attrs) {
            return;
        }
        let span = item.use_token.span;
        for (path, _) in flatten(&item.tree, Vec::new()) {
            self.record(&path, span);
        }
    }

    fn visit_path(&mut self, path: &'ast syn::Path) {
        let segments: Vec<_> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        self.record(&segments, path.segments[0].ident.span());
        syn::visit::visit_path(self, path);
    }

    fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
        if !is_test_only(&item.attrs) {
            syn::visit::visit_item_fn(self, item);
        }
    }

    fn visit_impl_item_fn(&mut self, item: &'ast syn::ImplItemFn) {
        if !is_test_only(&item.attrs) {
            syn::visit::visit_impl_item_fn(self, item);
        }
    }

    fn visit_item_mod(&mut self, _: &'ast syn::ItemMod) {
        // `collect_module` walks submodules with their own module path.
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        // Paths inside macro calls such as `matches!` and `format!` are
        // tokens, not syntax; parse what is a list of expressions.
        if let Ok(exprs) = mac.parse_body_with(
            syn::punctuated::Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated,
        ) {
            for expr in &exprs {
                self.visit_expr(expr);
            }
        }
        syn::visit::visit_macro(self, mac);
    }
}
