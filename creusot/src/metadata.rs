use crate::{
    creusot_items::CreusotItems, ctx::*, external::ExternSpec, translation::pearlite::Term,
};
use creusot_metadata::{
    decoder::{Decodable, MetadataBlob, MetadataDecoder},
    encoder::{Encodable, MetadataEncoder},
};
use indexmap::IndexMap;
use rustc_hir::def_id::{CrateNum, DefId, LOCAL_CRATE};
use rustc_macros::{TyDecodable, TyEncodable};
use rustc_middle::ty::TyCtxt;
use rustc_span::Symbol;
use std::{
    collections::HashMap,
    fs::File,
    io::Write,
    path::{Path, PathBuf},
};

type ExternSpecs<'tcx> = HashMap<DefId, ExternSpec<'tcx>>;

// TODO: this should lazily load the metadata.
#[derive(Default)]
pub struct Metadata<'tcx> {
    crates: HashMap<CrateNum, CrateMetadata<'tcx>>,
    extern_specs: ExternSpecs<'tcx>,
}

impl<'tcx> Metadata<'tcx> {
    pub(crate) fn get(&self, cnum: CrateNum) -> Option<&CrateMetadata<'tcx>> {
        self.crates.get(&cnum)
    }

    pub(crate) fn term(&self, def_id: DefId) -> Option<&Term<'tcx>> {
        assert!(!def_id.is_local());
        self.get(def_id.krate)?.term(def_id)
    }

    pub(crate) fn creusot_item(&self, sym: Symbol) -> Option<DefId> {
        for cmeta in &self.crates {
            if cmeta.1.creusot_item(sym).is_some() {
                return cmeta.1.creusot_item(sym);
            }
        }
        return None;
    }

    pub(crate) fn extern_spec(&self, id: DefId) -> Option<&ExternSpec<'tcx>> {
        self.extern_specs.get(&id)
    }

    pub(crate) fn load(&mut self, tcx: TyCtxt<'tcx>, overrides: &HashMap<String, String>) {
        for cnum in external_crates(tcx) {
            let (cmeta, mut ext_specs) = CrateMetadata::load(tcx, overrides, cnum);
            self.crates.insert(cnum, cmeta);

            for (id, spec) in ext_specs.drain() {
                if let Some(_) = self.extern_specs.insert(id, spec) {
                    panic!("duplicate external spec found for {:?} while loading {:?}", id, cnum);
                }
            }
        }
    }
}

pub struct CrateMetadata<'tcx> {
    terms: IndexMap<DefId, Term<'tcx>>,
    creusot_items: CreusotItems,
}

impl<'tcx> CrateMetadata<'tcx> {
    pub(crate) fn new() -> Self {
        Self { terms: Default::default(), creusot_items: Default::default() }
    }

    pub(crate) fn term(&self, def_id: DefId) -> Option<&Term<'tcx>> {
        assert!(!def_id.is_local());
        self.terms.get(&def_id)
    }

    pub(crate) fn creusot_item(&self, sym: Symbol) -> Option<DefId> {
        self.creusot_items.symbol_to_id.get(&sym).cloned()
    }

    fn load(
        tcx: TyCtxt<'tcx>,
        overrides: &HashMap<String, String>,
        cnum: CrateNum,
    ) -> (Self, ExternSpecs<'tcx>) {
        let mut meta = CrateMetadata::new();

        let base_path = creusot_metadata_base_path(tcx, overrides, cnum);

        let binary_path = creusot_metadata_binary_path(base_path.clone());

        let mut externs = Default::default();
        if let Some(metadata) = load_binary_metadata(tcx, cnum, &binary_path) {
            // for (def_id, summary) in metadata.dependencies.into_iter() {
            // meta.dependencies.insert(def_id, summary.into_iter().collect());
            // }

            for (def_id, summary) in metadata.terms.into_iter() {
                meta.terms.insert(def_id, summary);
            }

            meta.creusot_items = metadata.creusot_items;

            externs = metadata.extern_specs;
        }

        (meta, externs)
    }
}

// We use this type to perform (de)serialization of metadata because for annoying
// `extern crate` related reasons we cannot use the instance of `TyEncodable` / `TyDecodable`
// for `IndexMap`. Instead, we flatten it to a association list and then convert that into
// a proper index map after parsing.
#[derive(TyDecodable, TyEncodable)]
pub(crate) struct BinaryMetadata<'tcx> {
    terms: Vec<(DefId, Term<'tcx>)>,

    creusot_items: CreusotItems,

    extern_specs: HashMap<DefId, ExternSpec<'tcx>>,
}

impl<'tcx> BinaryMetadata<'tcx> {
    pub(crate) fn from_parts(
        terms: &IndexMap<DefId, Term<'tcx>>,
        items: &CreusotItems,
        extern_specs: &HashMap<DefId, ExternSpec<'tcx>>,
    ) -> Self {
        let terms = terms
            .iter()
            .filter(|(def_id, _)| def_id.is_local())
            .map(|(id, t)| (*id, t.clone()))
            .collect();

        BinaryMetadata { terms, creusot_items: items.clone(), extern_specs: extern_specs.clone() }
    }
}

fn export_file(ctx: &TranslationCtx, out: &Option<String>) -> PathBuf {
    out.as_ref().map(|s| s.clone().into()).unwrap_or_else(|| {
        let outputs = ctx.tcx.output_filenames(());

        let crate_name = ctx.tcx.crate_name(LOCAL_CRATE);
        let libname = format!("{}{}", crate_name.as_str(), ctx.tcx.sess.opts.cg.extra_filename);

        outputs.out_directory.join(&format!("lib{}.cmeta", libname))
    })
}

pub(crate) fn dump_exports(ctx: &TranslationCtx, out: &Option<String>) {
    let out_filename = export_file(ctx, out);
    debug!("dump_exports={:?}", out_filename);

    dump_binary_metadata(ctx.tcx, &out_filename, ctx.metadata()).unwrap_or_else(|err| {
        panic!("could not save metadata path=`{:?}` error={}", out_filename, err)
    });
}

fn dump_binary_metadata<'tcx>(
    tcx: TyCtxt<'tcx>,
    path: &Path,
    dep_info: BinaryMetadata<'tcx>,
) -> Result<(), std::io::Error> {
    let mut encoder = MetadataEncoder::new(tcx);
    dep_info.encode(&mut encoder);

    File::create(path).and_then(|mut file| file.write(&encoder.finish())).map_err(|err| {
        warn!("could not encode metadata for crate `{:?}`, error: {:?}", "LOCAL_CRATE", err);
        err
    })?;
    Ok(())
}

fn load_binary_metadata<'tcx>(
    tcx: TyCtxt<'tcx>,
    cnum: CrateNum,
    path: &Path,
) -> Option<BinaryMetadata<'tcx>> {
    let metadata = MetadataBlob::from_file(&path).and_then(|blob| {
        let mut decoder = MetadataDecoder::new(tcx, &blob);
        Ok(BinaryMetadata::decode(&mut decoder))
    });

    match metadata {
        Ok(b) => Some(b),
        Err(e) => {
            warn!("could not read metadata for crate `{:?}`: {:?}", tcx.crate_name(cnum), e);
            return None;
        }
    }
}

fn creusot_metadata_base_path(
    tcx: TyCtxt,
    overrides: &HashMap<String, String>,
    cnum: CrateNum,
) -> PathBuf {
    if let Some(path) = overrides.get(&tcx.crate_name(cnum).to_string()) {
        debug!("loading crate {:?} from override path", cnum);
        path.into()
    } else {
        let cs = tcx.used_crate_source(cnum);
        let x = cs.paths().next().unwrap().clone();
        x
    }
}

fn creusot_metadata_binary_path(mut path: PathBuf) -> PathBuf {
    path.set_extension("cmeta");
    path
}

fn external_crates(tcx: TyCtxt<'_>) -> Vec<CrateNum> {
    let mut deps = Vec::new();
    for cr in tcx.crates(()) {
        if let Some(extern_crate) = tcx.extern_crate(cr.as_def_id()) {
            if extern_crate.is_direct() {
                deps.push(*cr);
            }
        }
    }
    deps
}
