//! Rendering rustdoc type/signature data into concise, Rust-source-like
//! strings for search and display. Deliberately approximate: this is a
//! *concise API representation* (docs/design.md), not a pretty-printer.
//! Where-clauses are omitted to keep signatures short.

use rustdoc_types::{
    Abi, AssocItemConstraintKind, Constant, DynTrait, Function, GenericArg, GenericArgs,
    GenericBound, GenericParamDef, GenericParamDefKind, Path, PolyTrait, Term, Type,
};

/// Renders a concise signature for a function item, e.g.
/// pub async fn offload_blocking<F, R>(work: F) -> R
fn push_abi(out: &mut String, name: &str, unwind: bool) {
    out.push_str("extern ");
    out.push_str(name);
    if unwind {
        out.push_str("-unwind");
    }
    out.push(' ');
}

pub fn render_function(name: &str, f: &Function, public: bool) -> String {
    let mut out = String::new();
    if public {
        out.push_str("pub ");
    }
    let h = &f.header;
    if h.is_const {
        out.push_str("const ");
    }
    if h.is_unsafe {
        out.push_str("unsafe ");
    }
    if h.is_async {
        out.push_str("async ");
    }
    match &h.abi {
        Abi::Rust => {}
        Abi::C { unwind } => push_abi(&mut out, "C", *unwind),
        Abi::Cdecl { unwind } => push_abi(&mut out, "cdecl", *unwind),
        Abi::Stdcall { unwind } => push_abi(&mut out, "stdcall", *unwind),
        Abi::Fastcall { unwind } => push_abi(&mut out, "fastcall", *unwind),
        Abi::Aapcs { unwind } => push_abi(&mut out, "aapcs", *unwind),
        Abi::Win64 { unwind } => push_abi(&mut out, "win64", *unwind),
        Abi::SysV64 { unwind } => push_abi(&mut out, "sysv64", *unwind),
        Abi::System { unwind } => push_abi(&mut out, "system", *unwind),
        Abi::Other(s) => {
            out.push_str("extern ");
            out.push_str(s);
            out.push(' ');
        }
    }
    out.push_str("fn ");
    out.push_str(name);
    out.push_str(&render_generics_params(&f.generics.params));
    out.push('(');
    let inputs: Vec<String> = f
        .sig
        .inputs
        .iter()
        .map(|(arg, ty)| format!("{arg}: {}", render_type(ty)))
        .collect();
    out.push_str(&inputs.join(", "));
    if f.sig.is_c_variadic {
        if !f.sig.inputs.is_empty() {
            out.push_str(", ");
        }
        out.push_str("...");
    }
    out.push(')');
    if let Some(output) = &f.sig.output {
        out.push_str(" -> ");
        out.push_str(&render_type(output));
    }
    out
}

fn render_generics_params(params: &[GenericParamDef]) -> String {
    let rendered: Vec<String> = params
        .iter()
        .filter(|p| !is_synthetic_param(p))
        .map(render_generic_param)
        .collect();
    if rendered.is_empty() {
        String::new()
    } else {
        format!("<{}>", rendered.join(", "))
    }
}

fn is_synthetic_param(p: &GenericParamDef) -> bool {
    matches!(
        &p.kind,
        GenericParamDefKind::Type {
            is_synthetic: true,
            ..
        }
    )
}

fn render_generic_param(p: &GenericParamDef) -> String {
    match &p.kind {
        GenericParamDefKind::Lifetime { outlives } => {
            if outlives.is_empty() {
                p.name.clone()
            } else {
                format!("{}: {}", p.name, outlives.join(" + "))
            }
        }
        GenericParamDefKind::Type { bounds, .. } => {
            let bounds = render_bounds(bounds);
            if bounds.is_empty() {
                p.name.clone()
            } else {
                format!("{}: {bounds}", p.name)
            }
        }
        GenericParamDefKind::Const { type_, .. } => {
            format!("const {}: {}", p.name, render_type(type_))
        }
    }
}

pub fn render_bounds(bounds: &[GenericBound]) -> String {
    bounds
        .iter()
        .map(render_bound)
        .filter(|b| !b.is_empty())
        .collect::<Vec<_>>()
        .join(" + ")
}

fn render_bound(b: &GenericBound) -> String {
    match b {
        GenericBound::TraitBound { trait_, .. } => render_path(trait_),
        GenericBound::Outlives(l) => l.clone(),
        GenericBound::Use(_) => "use<..>".to_string(),
    }
}

pub fn render_path(path: &Path) -> String {
    let mut out = path.path.clone();
    if let Some(args) = &path.args {
        match args.as_ref() {
            GenericArgs::AngleBracketed { args, constraints } => {
                let mut parts: Vec<String> = args.iter().map(render_generic_arg).collect();
                for c in constraints {
                    let rendered = match &c.binding {
                        AssocItemConstraintKind::Equality(term) => {
                            format!("{} = {}", c.name, render_term(term))
                        }
                        AssocItemConstraintKind::Constraint(bounds) => {
                            format!("{}: {}", c.name, render_bounds(bounds))
                        }
                    };
                    parts.push(rendered);
                }
                if !parts.is_empty() {
                    out.push('<');
                    out.push_str(&parts.join(", "));
                    out.push('>');
                }
            }
            GenericArgs::Parenthesized { inputs, output } => {
                let inputs: Vec<String> = inputs.iter().map(render_type).collect();
                out.push('(');
                out.push_str(&inputs.join(", "));
                out.push(')');
                if let Some(output) = output {
                    out.push_str(" -> ");
                    out.push_str(&render_type(output));
                }
            }
            GenericArgs::ReturnTypeNotation => {}
        }
    }
    out
}

fn render_generic_arg(arg: &GenericArg) -> String {
    match arg {
        GenericArg::Lifetime(l) => l.clone(),
        GenericArg::Type(t) => render_type(t),
        GenericArg::Const(c) => render_const_value(c),
        GenericArg::Infer => "_".into(),
    }
}

fn render_const_value(c: &Constant) -> String {
    if let Some(value) = &c.value {
        value.clone()
    } else {
        c.expr.clone()
    }
}

fn render_term(term: &Term) -> String {
    match term {
        Term::Type(t) => render_type(t),
        Term::Constant(c) => render_const_value(c),
    }
}

pub fn render_type(t: &Type) -> String {
    match t {
        Type::ResolvedPath(p) => render_path(p),
        Type::DynTrait(d) => render_dyn(d),
        Type::Generic(s) => s.clone(),
        Type::Primitive(s) => s.clone(),
        Type::FunctionPointer(fp) => {
            let inputs: Vec<String> = fp
                .sig
                .inputs
                .iter()
                .map(|(_, ty)| render_type(ty))
                .collect();
            let mut out = format!("fn({})", inputs.join(", "));
            if let Some(output) = &fp.sig.output {
                out.push_str(" -> ");
                out.push_str(&render_type(output));
            }
            out
        }
        Type::Tuple(ts) => {
            let inner: Vec<String> = ts.iter().map(render_type).collect();
            format!("({})", inner.join(", "))
        }
        Type::Slice(inner) => format!("[{}]", render_type(inner)),
        Type::Array { type_, len } => format!("[{ty}; {len}]", ty = render_type(type_)),
        Type::Pat { type_, .. } => render_type(type_),
        Type::ImplTrait(bounds) => {
            let rendered = render_bounds(bounds);
            if rendered.is_empty() {
                "impl Trait".into()
            } else {
                format!("impl {rendered}")
            }
        }
        Type::Infer => "_".into(),
        Type::RawPointer { is_mutable, type_ } => {
            if *is_mutable {
                format!("*mut {}", render_type(type_))
            } else {
                format!("*const {}", render_type(type_))
            }
        }
        Type::BorrowedRef {
            lifetime,
            is_mutable,
            type_,
        } => {
            let mut out = String::from("&");
            if let Some(l) = lifetime {
                out.push('\'');
                out.push_str(l);
                out.push(' ');
            }
            if *is_mutable {
                out.push_str("mut ");
            }
            out.push_str(&render_type(type_));
            out
        }
        Type::QualifiedPath {
            name,
            args,
            self_type,
            trait_,
        } => {
            let base = match trait_ {
                Some(path) => {
                    format!("{} as {}", render_type(self_type), render_path(path))
                }
                None => render_type(self_type),
            };
            let mut out = format!("<{base}>::{name}");
            if let Some(args) = args {
                let parts: Vec<String> = match args.as_ref() {
                    GenericArgs::AngleBracketed { args, .. } => {
                        args.iter().map(render_generic_arg).collect()
                    }
                    GenericArgs::Parenthesized { .. } | GenericArgs::ReturnTypeNotation => {
                        Vec::new()
                    }
                };
                if !parts.is_empty() {
                    out.push('<');
                    out.push_str(&parts.join(", "));
                    out.push('>');
                }
            }
            out
        }
    }
}

fn render_dyn(d: &DynTrait) -> String {
    let traits: Vec<String> = d
        .traits
        .iter()
        .map(|p: &PolyTrait| render_path(&p.trait_))
        .collect();
    let joined = traits.join(" + ");
    match &d.lifetime {
        Some(l) => format!("dyn {joined} + {l}"),
        None => format!("dyn {joined}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustdoc_types::{FunctionHeader, FunctionSignature, Generics};

    fn function(inputs: Vec<(&str, Type)>, output: Option<Type>) -> Function {
        Function {
            sig: FunctionSignature {
                inputs: inputs
                    .into_iter()
                    .map(|(n, t)| (n.to_string(), t))
                    .collect(),
                output,
                is_c_variadic: false,
            },
            generics: Generics {
                params: vec![],
                where_predicates: vec![],
            },
            header: FunctionHeader {
                is_const: false,
                is_unsafe: false,
                is_async: false,
                abi: Abi::Rust,
            },
            has_body: true,
            default_unstable: None,
        }
    }

    #[test]
    fn renders_simple_function() {
        let f = function(
            vec![
                ("self", Type::Generic("Self".into())),
                (
                    "data",
                    Type::BorrowedRef {
                        lifetime: None,
                        is_mutable: false,
                        type_: Box::new(Type::Slice(Box::new(Type::Primitive("u8".into())))),
                    },
                ),
            ],
            Some(Type::Primitive("usize".into())),
        );
        assert_eq!(
            render_function("write_all", &f, true),
            "pub fn write_all(self: Self, data: &[u8]) -> usize"
        );
    }

    #[test]
    fn renders_borrowed_ref_and_option() {
        let ref_type = Type::BorrowedRef {
            lifetime: Some("a".into()),
            is_mutable: true,
            type_: Box::new(Type::Primitive("i32".into())),
        };
        let f = function(vec![("x", ref_type)], None);
        assert_eq!(
            render_function("borrow_mut", &f, true),
            "pub fn borrow_mut(x: &'a mut i32)"
        );
    }

    #[test]
    fn renders_async_function() {
        let mut f = function(vec![], None);
        f.header.is_async = true;
        assert_eq!(render_function("run", &f, true), "pub async fn run()");
    }
}
