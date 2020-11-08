use std::collections::HashMap;

use rustc_middle::{
    mir::{BasicBlockData, Place, Rvalue, StatementKind as StmtK, TerminatorKind},
    ty::{Ty, TyCtxt, VariantDef},
};

use crate::mlcfg::{MlCfgConstant, MlCfgPattern};

// Find the place being discriminated, if there is one
pub fn discriminator_for_switch<'tcx>(bbd: &BasicBlockData<'tcx>) -> Option<Place<'tcx>> {
    let discr = if let TerminatorKind::SwitchInt { discr, .. } = &bbd.terminator().kind {
        discr
    } else {
        return None;
    };

    if let StmtK::Assign(box (pl, Rvalue::Discriminant(real_discr))) =
        bbd.statements.last().unwrap().kind
    {
        if discr.place() == Some(pl) {
            Some(real_discr)
        } else {
            None
        }
    } else {
        None
    }
}

// Create the patterns for a switch
pub fn branches_for_ty<'tcx>(
    tcx: TyCtxt<'tcx>,
    switch_ty: Ty<'tcx>,
    targets: Vec<u128>,
) -> Vec<MlCfgPattern> {
    use rustc_middle::ty::TyKind::*;
    match switch_ty.kind() {
        Adt(def, _) => {
            let discr_to_var_idx: HashMap<_, _> =
                def.discriminants(tcx).map(|(idx, d)| (d.val, idx)).collect();

            targets
                .iter()
                .map(|disc| variant_pattern(&def.variants[discr_to_var_idx[&disc]]))
                .collect()
        }
        Tuple(_) => unimplemented!("tuple"),
        Bool => vec![
            MlCfgPattern::LitP(MlCfgConstant::const_false()),
            MlCfgPattern::LitP(MlCfgConstant::const_true()),
        ],
        _ => unimplemented!("constant pattern"),
    }
}

pub fn variant_pattern(var: &VariantDef) -> MlCfgPattern {
    let wilds = var.fields.iter().map(|_| MlCfgPattern::Wildcard).collect();
    let cons_name = var.ident.to_string();
    MlCfgPattern::ConsP(cons_name, wilds)
}