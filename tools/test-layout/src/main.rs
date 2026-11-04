use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::{Attribute, Item};
use walkdir::WalkDir;

struct LayoutVisitor<'a> {
    path: &'a Path,
    violations: Vec<String>,
}

impl<'a> LayoutVisitor<'a> {
    fn inspect(&mut self, item: &Item) {
        let attrs = item_attributes(item);
        if !attrs.iter().any(|attr| attribute_mentions_test(attr)) {
            return;
        }

        if is_allowed_tests_module(item, attrs) {
            return;
        }

        let line = item.span().start().line;
        self.violations.push(format!(
            "{}:{}: {}",
            self.path.display(),
            line,
            violation_reason(item)
        ));
    }
}

impl<'ast> Visit<'ast> for LayoutVisitor<'_> {
    fn visit_item(&mut self, item: &'ast Item) {
        self.inspect(item);
        visit::visit_item(self, item);
    }
}

fn item_attributes(item: &Item) -> &[Attribute] {
    match item {
        Item::Const(item) => &item.attrs,
        Item::Enum(item) => &item.attrs,
        Item::ExternCrate(item) => &item.attrs,
        Item::Fn(item) => &item.attrs,
        Item::ForeignMod(item) => &item.attrs,
        Item::Impl(item) => &item.attrs,
        Item::Macro(item) => &item.attrs,
        Item::Mod(item) => &item.attrs,
        Item::Static(item) => &item.attrs,
        Item::Struct(item) => &item.attrs,
        Item::Trait(item) => &item.attrs,
        Item::TraitAlias(item) => &item.attrs,
        Item::Type(item) => &item.attrs,
        Item::Union(item) => &item.attrs,
        Item::Use(item) => &item.attrs,
        Item::Verbatim(_) => &[],
        _ => &[],
    }
}

fn attribute_mentions_test(attr: &Attribute) -> bool {
    if !(attr.path().is_ident("cfg") || attr.path().is_ident("cfg_attr")) {
        return false;
    }

    match &attr.meta {
        syn::Meta::List(meta) => meta
            .tokens
            .to_string()
            .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
            .any(|token| token == "test"),
        _ => false,
    }
}

fn is_exact_cfg_test(attr: &Attribute) -> bool {
    match &attr.meta {
        syn::Meta::List(meta) => {
            meta.path.is_ident("cfg") && meta.tokens.to_string().trim() == "test"
        }
        _ => false,
    }
}

fn is_allowed_tests_module(item: &Item, attrs: &[Attribute]) -> bool {
    let Item::Mod(module) = item else {
        return false;
    };
    module.ident == "tests"
        && module.content.is_none()
        && attrs.len() == 1
        && is_exact_cfg_test(&attrs[0])
}

fn violation_reason(item: &Item) -> &'static str {
    match item {
        Item::Mod(module) if module.content.is_some() => "inline test module",
        Item::Mod(_) => "test-only module must be exactly `#[cfg(test)] mod tests;`",
        Item::Fn(_) => "test-only function",
        Item::Use(_) => "test-only import",
        Item::Const(_) => "test-only constant",
        Item::Static(_) => "test-only static",
        Item::Impl(_) => "test-only impl",
        Item::Macro(_) => "test-only macro",
        _ => "test-only production item",
    }
}

fn rust_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    for entry in WalkDir::new(root).follow_links(false) {
        let entry = entry.map_err(|error| format!("walk {}: {error}", root.display()))?;
        if entry.file_type().is_file() && entry.path().extension() == Some(OsStr::new("rs")) {
            files.push(entry.into_path());
        }
    }
    files.sort();
    Ok(files)
}

fn check(root: &Path) -> Result<Vec<String>, String> {
    if !root.is_dir() {
        return Err(format!(
            "source root is not a directory: {}",
            root.display()
        ));
    }

    let mut violations = Vec::new();
    for path in rust_files(root)? {
        let source = fs::read_to_string(&path)
            .map_err(|error| format!("read {}: {error}", path.display()))?;
        let syntax = syn::parse_file(&source)
            .map_err(|error| format!("parse {}: {error}", path.display()))?;
        let mut visitor = LayoutVisitor {
            path: &path,
            violations: Vec::new(),
        };
        visitor.visit_file(&syntax);
        violations.extend(visitor.violations);
    }
    Ok(violations)
}

fn main() {
    let mut args = env::args_os().skip(1);
    let Some(root) = args.next() else {
        eprintln!("usage: pactree-test-layout <source-root>");
        std::process::exit(2);
    };
    if args.next().is_some() {
        eprintln!("usage: pactree-test-layout <source-root>");
        std::process::exit(2);
    }

    match check(Path::new(&root)) {
        Ok(violations) if violations.is_empty() => {}
        Ok(violations) => {
            for violation in violations {
                eprintln!("{violation}");
            }
            std::process::exit(1);
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    }
}
