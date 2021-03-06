use crate::check::{FnCtxt, LocalTy, UserType};
use rustc_hir as hir;
use rustc_hir::intravisit::{self, NestedVisitorMap, Visitor};
use rustc_hir::PatKind;
use rustc_infer::infer::type_variable::{TypeVariableOrigin, TypeVariableOriginKind};
use rustc_middle::ty::Ty;
use rustc_span::{sym, Span};
use rustc_trait_selection::traits;

pub(super) struct GatherLocalsVisitor<'a, 'tcx> {
    fcx: &'a FnCtxt<'a, 'tcx>,
    parent_id: hir::HirId,
    // parameters are special cases of patterns, but we want to handle them as
    // *distinct* cases. so track when we are hitting a pattern *within* an fn
    // parameter.
    outermost_fn_param_pat: Option<Span>,
}

impl<'a, 'tcx> GatherLocalsVisitor<'a, 'tcx> {
    pub(super) fn new(fcx: &'a FnCtxt<'a, 'tcx>, parent_id: hir::HirId) -> Self {
        Self { fcx, parent_id, outermost_fn_param_pat: None }
    }

    fn assign(&mut self, span: Span, nid: hir::HirId, ty_opt: Option<LocalTy<'tcx>>) -> Ty<'tcx> {
        match ty_opt {
            None => {
                // Infer the variable's type.
                let var_ty = self.fcx.next_ty_var(TypeVariableOrigin {
                    kind: TypeVariableOriginKind::TypeInference,
                    span,
                });
                self.fcx
                    .locals
                    .borrow_mut()
                    .insert(nid, LocalTy { decl_ty: var_ty, revealed_ty: var_ty });
                var_ty
            }
            Some(typ) => {
                // Take type that the user specified.
                self.fcx.locals.borrow_mut().insert(nid, typ);
                typ.revealed_ty
            }
        }
    }
}

impl<'a, 'tcx> Visitor<'tcx> for GatherLocalsVisitor<'a, 'tcx> {
    type Map = intravisit::ErasedMap<'tcx>;

    fn nested_visit_map(&mut self) -> NestedVisitorMap<Self::Map> {
        NestedVisitorMap::None
    }

    // Add explicitly-declared locals.
    fn visit_local(&mut self, local: &'tcx hir::Local<'tcx>) {
        let local_ty = match local.ty {
            Some(ref ty) => {
                let o_ty = self.fcx.to_ty(&ty);

                let revealed_ty = self.fcx.instantiate_opaque_types_from_value(
                    self.parent_id,
                    o_ty,
                    ty.span,
                    Some(sym::impl_trait_in_bindings),
                );

                let c_ty =
                    self.fcx.inh.infcx.canonicalize_user_type_annotation(UserType::Ty(revealed_ty));
                debug!(
                    "visit_local: ty.hir_id={:?} o_ty={:?} revealed_ty={:?} c_ty={:?}",
                    ty.hir_id, o_ty, revealed_ty, c_ty
                );
                self.fcx
                    .typeck_results
                    .borrow_mut()
                    .user_provided_types_mut()
                    .insert(ty.hir_id, c_ty);

                Some(LocalTy { decl_ty: o_ty, revealed_ty })
            }
            None => None,
        };
        self.assign(local.span, local.hir_id, local_ty);

        debug!(
            "local variable {:?} is assigned type {}",
            local.pat,
            self.fcx.ty_to_string(&*self.fcx.locals.borrow().get(&local.hir_id).unwrap().decl_ty)
        );
        intravisit::walk_local(self, local);
    }

    fn visit_param(&mut self, param: &'tcx hir::Param<'tcx>) {
        let old_outermost_fn_param_pat = self.outermost_fn_param_pat.replace(param.ty_span);
        intravisit::walk_param(self, param);
        self.outermost_fn_param_pat = old_outermost_fn_param_pat;
    }

    // Add pattern bindings.
    fn visit_pat(&mut self, p: &'tcx hir::Pat<'tcx>) {
        if let PatKind::Binding(_, _, ident, _) = p.kind {
            let var_ty = self.assign(p.span, p.hir_id, None);

            if let Some(ty_span) = self.outermost_fn_param_pat {
                if !self.fcx.tcx.features().unsized_fn_params {
                    self.fcx.require_type_is_sized(
                        var_ty,
                        p.span,
                        traits::SizedArgumentType(Some(ty_span)),
                    );
                }
            } else {
                if !self.fcx.tcx.features().unsized_locals {
                    self.fcx.require_type_is_sized(var_ty, p.span, traits::VariableType(p.hir_id));
                }
            }

            debug!(
                "pattern binding {} is assigned to {} with type {:?}",
                ident,
                self.fcx.ty_to_string(&*self.fcx.locals.borrow().get(&p.hir_id).unwrap().decl_ty),
                var_ty
            );
        }
        let old_outermost_fn_param_pat = self.outermost_fn_param_pat.take();
        intravisit::walk_pat(self, p);
        self.outermost_fn_param_pat = old_outermost_fn_param_pat;
    }

    // Don't descend into the bodies of nested closures.
    fn visit_fn(
        &mut self,
        _: intravisit::FnKind<'tcx>,
        _: &'tcx hir::FnDecl<'tcx>,
        _: hir::BodyId,
        _: Span,
        _: hir::HirId,
    ) {
    }
}
