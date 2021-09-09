use indexmap::{IndexMap, IndexSet};

use why3::declaration::{Module, TyDecl};
use why3::QName;

use rustc_errors::DiagnosticId;
use rustc_hir::def_id::DefId;
use rustc_middle::ty::{self, TyCtxt};
use rustc_session::Session;
use rustc_span::Span;

use rustc_hir::def::DefKind;
use rustc_hir::definitions::DefPathData;
use rustc_resolve::Namespace;

use crate::{options::Options, util};

pub use crate::clone_map::*;

pub struct TranslationCtx<'sess, 'tcx> {
    pub sess: &'sess Session,
    pub tcx: TyCtxt<'tcx>,
    pub translated_items: IndexSet<DefId>,
    pub types: Vec<TyDecl>,
    pub functions: IndexMap<DefId, Module>,
    pub externs: IndexMap<DefId, Module>,
    pub opts: &'sess Options,
}

impl<'tcx, 'sess> TranslationCtx<'sess, 'tcx> {
    pub fn new(tcx: TyCtxt<'tcx>, sess: &'sess Session, opts: &'sess Options) -> Self {
        Self {
            sess,
            tcx,
            translated_items: IndexSet::new(),
            types: Vec::new(),
            functions: IndexMap::new(),
            externs: IndexMap::new(),
            opts,
        }
    }

    // Generic entry point for function translation
    pub fn translate_function(&mut self, def_id: DefId) {
        if !self.translated_items.insert(def_id) {
            return;
        }

        if self.tcx.trait_of_item(def_id).is_some() {
            return;
        }

        if !crate::util::should_translate(self.tcx, def_id) {
            info!("Skipping {:?}", def_id);
            return;
        }

        let span = self.tcx.hir().span_if_local(def_id).unwrap_or(rustc_span::DUMMY_SP);
        let module = if !def_id.is_local() {
            debug!("translating {:?} as extern", def_id);
            crate::translation::translate_extern(self, def_id, span)
        } else if crate::translation::is_logic(self.tcx, def_id) {
            debug!("translating {:?} as logic", def_id);
            crate::translation::translate_logic(self, def_id, span)
        } else if crate::translation::is_predicate(self.tcx, def_id) {
            debug!("translating {:?} as predicate", def_id);
            crate::translation::translate_predicate(self, def_id, span)
        } else {
            let kind = self.tcx.def_kind(def_id);
            if kind == DefKind::Fn || kind == DefKind::Closure || kind == DefKind::AssocFn {
                let is_spec = util::is_invariant(self.tcx, def_id);
                if is_spec {
                    return;
                }

                crate::translation::translate_function(self.tcx, self, def_id)
            } else {
                unimplemented!("{:?} {:?}", def_id, self.tcx.def_kind(def_id))
            }
        };

        self.functions.insert(def_id, module);
    }

    pub fn crash_and_error(&self, span: Span, msg: &str) -> ! {
        self.sess.span_fatal_with_code(span, msg, DiagnosticId::Error(String::from("creusot")))
    }

    pub fn warn(&self, span: Span, msg: &str) {
        self.sess.span_warn_with_code(
            span,
            msg,
            DiagnosticId::Lint {
                name: String::from("creusot"),
                has_future_breakage: false,
                is_force_warn: true,
            },
        )
    }

    pub fn add_type(&mut self, decl: TyDecl) {
        let mut dependencies = decl.used_types();
        let mut pos = 0;
        while !dependencies.is_empty() && pos < self.types.len() {
            dependencies.remove(&self.types[pos].ty_name);
            pos += 1;
        }

        self.types.insert(pos, decl);
    }

    pub fn should_export(&self) -> bool {
        self.opts.export_metadata
    }

    pub fn should_compile(&self) -> bool {
        !self.opts.dependency
    }
}

use heck::CamelCase;

pub fn translate_type_id(tcx: TyCtxt, def_id: DefId) -> QName {
    translate_defid(tcx, def_id, true)
}

pub fn translate_value_id(tcx: TyCtxt, def_id: DefId) -> QName {
    translate_defid(tcx, def_id, false)
}

pub fn cloneable_name(tcx: TyCtxt, def_id: DefId) -> QName {
    let qname = translate_value_id(tcx, def_id);
    use rustc_hir::def::DefKind::*;

    match tcx.def_kind(def_id) {
        Trait => qname,
        _ => qname.module_name(),
    }
}

pub(crate) fn method_name(tcx: TyCtxt, def_id: DefId) -> String {
    tcx.item_name(def_id).to_string().to_lowercase()
}

fn translate_defid(tcx: TyCtxt, def_id: DefId, ty: bool) -> QName {
    let def_path = tcx.def_path(def_id);

    let mut segments = Vec::new();

    let mut crate_name = tcx.crate_name(def_id.krate).to_string().to_camel_case();
    if crate_name.chars().nth(0).unwrap().is_numeric() {
        crate_name = format!("C{}", crate_name);
    }

    segments.push(crate_name);

    for seg in def_path.data[..].iter() {
        match seg.data {
            // DefPathData::CrateRoot => segments.push(tcx.crate_name(def_id.krate).to_string()),
            // CORE ASSUMPTION: Once we stop seeing TypeNs we never see it again.
            DefPathData::Ctor => {}
            _ => segments.push(format!("{}", seg).to_camel_case()),
        }
    }

    let kind = tcx.def_kind(def_id);
    use rustc_hir::def::DefKind::*;

    let name;
    match (kind, kind.ns()) {
        (_, _) if ty => {
            name = segments.into_iter().map(|seg| seg.to_lowercase()).collect();
            segments = vec!["Type".to_owned()];
        }
        (Ctor(_, _) | Variant | Struct | Enum, _) => {
            segments[0] = segments[0].to_camel_case();
            name = segments;
            segments = vec!["Type".to_owned()];
        }
        (Trait | Mod | Impl, _) => {
            name = segments;
            segments = Vec::new();
        }
        (_, Some(Namespace::ValueNS)) | (Closure, _) => {
            let is_trait_assoc = tcx.trait_of_item(def_id).is_some();

            if is_trait_assoc {
                segments.pop().unwrap();
            }

            name = vec![method_name(tcx, def_id)];
        }
        (a, b) => unreachable!("{:?} {:?} {:?}", a, b, segments),
    }
    let module = if segments.is_empty() { Vec::new() } else { vec![segments.join("_")] };

    QName { module, name: name.join("_") }
}