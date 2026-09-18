#[allow(dead_code)]
#[path = "src/locale_catalog.rs"]
mod locale_catalog;

fn validate_locales() {
    use syn::visit::Visit;
    let directory = std::path::Path::new("assets/locales");
    println!("cargo:rerun-if-changed=assets/locales");
    println!("cargo:rerun-if-changed=src");
    let en =
        locale_catalog::parse_pack(&std::fs::read_to_string(directory.join("en.json")).unwrap())
            .expect("invalid en.json");
    let zh =
        locale_catalog::parse_pack(&std::fs::read_to_string(directory.join("zh-CN.json")).unwrap())
            .expect("invalid zh-CN.json");
    let aliases = locale_catalog::parse_aliases(
        &std::fs::read_to_string(directory.join("legacy-keys.json")).unwrap(),
    )
    .expect("invalid aliases");
    locale_catalog::validate_catalog(&en, &zh, &aliases)
        .expect("language catalog validation failed");
    assert_eq!(en.id, "en");
    assert_eq!(zh.id, "zh-CN");
    let namespaces = en
        .strings
        .keys()
        .map(|key| key.split('.').next().unwrap().to_owned())
        .collect::<std::collections::BTreeSet<_>>();
    struct Validator<'a> {
        keys: &'a locale_catalog::Strings,
        namespaces: &'a std::collections::BTreeSet<String>,
        file: String,
        errors: Vec<String>,
    }
    impl<'ast> Visit<'ast> for Validator<'_> {
        fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
            if ["title", "label", "tooltip", "child", "placeholder"]
                .iter()
                .any(|name| node.method == *name)
                && let Some(syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(text),
                    ..
                })) = node.args.first()
                && self.keys.contains_key(&text.value())
            {
                self.errors.push(format!(
                    "{}: semantic key {} must be translated before rendering",
                    self.file,
                    text.value()
                ));
            }
            syn::visit::visit_expr_method_call(self, node);
        }
        fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
            if node.attrs.iter().any(|attr| {
                attr.path().is_ident("cfg")
                    && matches!(&attr.meta,syn::Meta::List(meta) if meta.tokens.to_string()=="test")
            }) {
                return;
            }
            syn::visit::visit_item_mod(self, node);
        }
        fn visit_lit_str(&mut self, node: &'ast syn::LitStr) {
            let key = node.value();
            if locale_catalog::semantic_key(&key)
                && self.namespaces.contains(key.split('.').next().unwrap())
                && !self.keys.contains_key(&key)
            {
                self.errors
                    .push(format!("{}: undefined key {key}", self.file));
            }
        }
        fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
            if let syn::Expr::Path(path) = node.func.as_ref()
                && path
                    .path
                    .segments
                    .last()
                    .is_some_and(|s| s.ident == "t" || s.ident == "tr")
                && let Some(syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(key),
                    ..
                })) = node.args.iter().nth(1)
                && !self.keys.contains_key(&key.value())
            {
                self.errors.push(format!(
                    "{}: translation lookup must use a defined semantic key: {}",
                    self.file,
                    key.value()
                ));
            }
            if let syn::Expr::Path(path) = node.func.as_ref()
                && let Some(function) = path.path.segments.last()
                && (function.ident == "t" || function.ident == "tr")
                && let Some(syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(key),
                    ..
                })) = node.args.iter().nth(1)
                && let Some(template) = self.keys.get(&key.value())
            {
                let expected = locale_catalog::placeholders(template).expect("validated template");
                if function.ident == "t" && !expected.is_empty() {
                    self.errors.push(format!(
                        "{}: {} requires tr() with named arguments",
                        self.file,
                        key.value()
                    ));
                }
                if function.ident == "tr"
                    && let Some(syn::Expr::Reference(reference)) = node.args.iter().nth(2)
                    && let syn::Expr::Array(array) = reference.expr.as_ref()
                {
                    let provided = array
                        .elems
                        .iter()
                        .filter_map(|element| {
                            if let syn::Expr::Tuple(tuple) = element
                                && let Some(syn::Expr::Lit(syn::ExprLit {
                                    lit: syn::Lit::Str(name),
                                    ..
                                })) = tuple.elems.first()
                            {
                                Some(name.value())
                            } else {
                                None
                            }
                        })
                        .collect::<std::collections::BTreeSet<_>>();
                    if provided != expected || provided.len() != array.elems.len() {
                        self.errors.push(format!(
                            "{}: {} placeholder arguments differ",
                            self.file,
                            key.value()
                        ));
                    }
                }
            }
            syn::visit::visit_expr_call(self, node);
        }
    }
    let mut errors = vec![];
    for entry in std::fs::read_dir("src").unwrap().flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "rs") {
            continue;
        }
        let source = std::fs::read_to_string(&path).unwrap();
        let syntax = syn::parse_file(&source)
            .expect("unable to parse Rust source for translation validation");
        let mut validator = Validator {
            keys: &en.strings,
            namespaces: &namespaces,
            file: path.display().to_string(),
            errors: vec![],
        };
        validator.visit_file(&syntax);
        errors.extend(validator.errors);
    }
    assert!(
        errors.is_empty(),
        "Translation references invalid:\n{}",
        errors.join("\n")
    );
}

fn main() {
    validate_locales();
    println!("cargo:rerun-if-changed=assets/windows.rc");
    println!("cargo:rerun-if-changed=assets/turbodbnote.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("assets/windows.rc", embed_resource::NONE)
            .manifest_required()
            .expect("Unable to embed TurboDbNote Windows icon resource");
    }
}
