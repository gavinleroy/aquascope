//! Datalog analysis for Aquascope

use std::time::Instant;

use datafrog::{Relation, RelationLeaper, ValueFilter};
use flowistry::mir::utils::PlaceExt;
use polonius_engine::{Algorithm, FactTypes, Output as PEOutput};
use rustc_borrowck::{borrow_set::BorrowSet, consumers::BodyWithBorrowckFacts};
use rustc_data_structures::fx::{FxHashMap as HashMap, FxHashSet as HashSet};
use rustc_hir::{BodyId, Mutability};
use rustc_index::vec::IndexVec;
use rustc_middle::{
  mir::{Place, ProjectionElem},
  ty::TyCtxt,
};
use rustc_mir_dataflow::move_paths::MoveData;

use super::{
  context::PermissionsCtxt,
  places_conflict::{AccessDepth, PlaceConflictBias},
  AquascopeFacts, Loan, Path, Point,
};

// FIXME the HashMap should map to multiple loans, because at a
// given point a path could be refined my multiple loans even
// if we only care about a single (more recent).
#[derive(Debug)]
pub struct Output<T>
where
  T: FactTypes + std::fmt::Debug,
{
  // TODO(gavinleroy): I really want to rename cannot_XXX to
  // path_XXX_loan_refined_at which is more explicit that these
  // only hold data referring to a live Loan regions.
  /// .decl never_write(Path)
  ///
  /// never_write(Path) :-
  ///    is_direct(Path),
  ///    declared_readonly(Path).
  ///
  /// never_write(Path) :-
  ///    !is_direct(Path),
  ///    prefix_of(Prefix, Path),
  ///    is_immut_ref(Prefix).
  ///
  pub never_write: HashSet<T::Path>,

  /// .decl never_drop(Path)
  ///
  /// never_drop(Path) :-
  ///    !is_direct(Path).
  ///
  /// DEPRECATED: TODO remove
  pub never_drop: HashSet<T::Path>,

  /// .decl cannot_read(Path:path, Point:point)
  ///
  /// cannot_read(Path, Loan, Point) :-
  ///    path_moved_at(Path, Point);
  ///    loan_conflicts_with(Loan, Path),
  ///    loan_live_at(Loan, Point),
  ///    loan_mutable(Loan).
  ///
  pub cannot_read: HashMap<T::Point, HashMap<T::Path, T::Loan>>,

  /// .decl cannot_write(Path:path, Point:point)
  ///
  /// cannot_write(Path, Loan, Point) :-
  ///    path_moved_at(Path, Point);
  ///    loan_conflicts_with(Loan, Path),
  ///    loan_live_at(Loan, Point).
  ///
  pub cannot_write: HashMap<T::Point, HashMap<T::Path, T::Loan>>,

  /// .decl cannot_drop(Path, Loan, Point)
  ///
  /// cannot_drop(Path, Loan, Point)
  ///    path_moved_at(Path, Point);
  ///    loan_conflicts_with(Loan, Path),
  ///    loan_live_at(Loan, Point).
  ///
  pub cannot_drop: HashMap<T::Point, HashMap<T::Path, T::Loan>>,

  /// .decl path_maybe_uninitialized_on_entry(Point, Path)
  ///
  /// path_maybe_uninitialized_on_entry(Point1, Path) :-
  ///    path_maybe_uninitialized_on_exit(Point0, Path)
  ///    cfg_edge(Point0, Point1)
  ///
  pub path_maybe_uninitialized_on_entry: HashMap<T::Point, HashSet<T::Path>>,
}

impl Default for Output<AquascopeFacts> {
  fn default() -> Self {
    Output {
      cannot_read: HashMap::default(),
      cannot_write: HashMap::default(),
      cannot_drop: HashMap::default(),
      path_maybe_uninitialized_on_entry: HashMap::default(),
      // path_moved_at: HashMap::default(),
      never_write: HashSet::default(),
      never_drop: HashSet::default(),
    }
  }
}

pub fn derive_permission_facts(ctxt: &mut PermissionsCtxt) {
  let def_id = ctxt.tcx.hir().body_owner_def_id(ctxt.body_id);
  let body = &ctxt.body_with_facts.body;
  let tcx = ctxt.tcx;

  // We consider all place that are either:
  // 1. Internal to a local declaration.
  // 2. A path considered moveable by rustc.
  let places = body
    .local_decls
    .indices()
    .flat_map(|local| {
      Place::from_local(local, tcx).interior_paths(
        tcx,
        body,
        def_id.to_def_id(),
      )
    })
    .chain(ctxt.move_data.move_paths.iter().map(|v| v.place))
    .collect::<Vec<_>>();

  // Normalize all places and get the associated AquascopeFacts::Point,
  // any MIR place that is not initialized here could cause a panic later
  // in the pipeline if a transformation (path -> [point|moveable_path,...])
  // happens.
  let paths = places
    .iter()
    .map(|place| ctxt.new_path(*place))
    .collect::<Vec<_>>();

  let loan_to_borrow = |l: Loan| &ctxt.borrow_set[l];

  let is_never_write = |path: Path| {
    let place = &ctxt.path_to_place(path);
    (!place.is_indirect() && ctxt.is_declared_readonly(place)) || {
      // Iff there exists an immutable prefix it is also `never_write`
      place
        .iter_projections()
        .filter_map(|(prefix, elem)| {
          matches!(elem, ProjectionElem::Deref).then_some(prefix)
        })
        .any(|prefix| {
          let ty = prefix.ty(&body.local_decls, tcx).ty;
          match ty.ref_mutability() {
            Some(mutability) => mutability == Mutability::Not,
            // TODO: raw pointers, assume that they are always mutable
            None => false,
          }
        })
    }
  };

  let is_never_drop = |path: Path| ctxt.path_to_place(path).is_indirect();

  // .decl loan_conflicts_with(Loan, Path)
  let loan_conflicts_with: Relation<(Loan, Path)> = Relation::from_iter(
    ctxt.polonius_input_facts.loan_issued_at.iter().flat_map(
      |(_origin, loan, _point)| {
        let borrow = loan_to_borrow(*loan);
        places.iter().filter_map(|place| {
          super::places_conflict::borrow_conflicts_with_place(
            tcx,
            body,
            borrow.borrowed_place,
            borrow.kind,
            place.as_ref(),
            AccessDepth::Deep,
            PlaceConflictBias::Overlap,
          )
          .then_some((*loan, ctxt.place_to_path(place)))
        })
      },
    ),
  );

  let loan_live_at: Relation<(Loan, Point)> = Relation::from_iter(
    ctxt
      .polonius_output
      .loan_live_at
      .iter()
      .flat_map(|(point, values)| values.iter().map(|loan| (*loan, *point))),
  );

  let cannot_read: Relation<(Path, Loan, Point)> = Relation::from_leapjoin(
    &loan_conflicts_with,
    (
      loan_live_at.extend_with(|&(loan, _path)| loan),
      ValueFilter::from(|&(loan, _path), _point| ctxt.is_mutable_loan(loan)),
    ),
    |&(loan, path), &point| (path, loan, point),
  );

  let cannot_write: Relation<(Path, Loan, Point)> = Relation::from_join(
    &loan_conflicts_with,
    &loan_live_at,
    |&loan, &path, &point| (path, loan, point),
  );

  let cannot_drop: Relation<(Path, Loan, Point)> = Relation::from_join(
    &loan_conflicts_with,
    &loan_live_at,
    |&loan, &path, &point| (path, loan, point),
  );

  let never_write = paths
    .iter()
    .filter_map(|path| is_never_write(*path).then_some(*path))
    .collect::<HashSet<_>>();

  let never_drop = paths
    .iter()
    .filter_map(|path| is_never_drop(*path).then_some(*path))
    .collect::<HashSet<_>>();

  ctxt.permissions_output.never_write = never_write;
  ctxt.permissions_output.never_drop = never_drop;

  let cfg_edge: Relation<(Point, Point)> = Relation::from_iter(
    ctxt
      .polonius_input_facts
      .cfg_edge
      .iter()
      .map(|&(p1, p2)| (p1, p2)),
  );

  let path_maybe_uninitialized_on_exit: Relation<(Point, Path)> =
    Relation::from_iter(
      ctxt
        .polonius_output
        .path_maybe_uninitialized_on_exit
        .iter()
        .flat_map(|(point, paths)| {
          paths.iter().map(|path| {
            let path = ctxt.moveable_path_to_path(*path);
            (*point, path)
          })
        }),
    );

  let path_maybe_uninitialized_on_entry: Relation<(Point, Path)> =
    Relation::from_join(
      &path_maybe_uninitialized_on_exit,
      &cfg_edge,
      |&_point1, &path, &point2| (point2, path),
    );

  for &(point, path) in path_maybe_uninitialized_on_entry.iter() {
    ctxt
      .permissions_output
      .path_maybe_uninitialized_on_entry
      .entry(point)
      .or_default()
      .insert(path);
  }

  macro_rules! insert_facts {
    ($input:expr, $field:expr) => {
      for &(path, loan, point) in $input.iter() {
        $field.entry(point).or_default().insert(path, loan);
      }
    };
  }

  insert_facts!(cannot_read, ctxt.permissions_output.cannot_read);
  insert_facts!(cannot_write, ctxt.permissions_output.cannot_write);
  insert_facts!(cannot_drop, ctxt.permissions_output.cannot_drop);

  log::debug!(
    "#cannot_read {} #cannot_write {} #cannot_drop {}",
    cannot_read.len(),
    cannot_write.len(),
    cannot_drop.len()
  );
}

// ----------
// Main entry

pub fn compute<'a, 'tcx>(
  tcx: TyCtxt<'tcx>,
  body_id: BodyId,
  body_with_facts: &'a BodyWithBorrowckFacts<'tcx>,
) -> PermissionsCtxt<'a, 'tcx> {
  let timer = Instant::now();
  let def_id = tcx.hir().body_owner_def_id(body_id);
  let body = &body_with_facts.body;

  // for debugging pruposes only
  let owner = tcx.hir().body_owner(body_id);
  let name = match tcx.hir().opt_name(owner) {
    Some(name) => name.to_ident_string(),
    None => "<anonymous>".to_owned(),
  };
  log::debug!("computing body permissions {:?}", name);

  let polonius_input_facts = &body_with_facts.input_facts;
  let polonius_output =
    PEOutput::compute(polonius_input_facts, Algorithm::Naive, true);

  let locals_are_invalidated_at_exit =
    tcx.hir().body_owner_kind(def_id).is_fn_or_closure();
  let move_data = match MoveData::gather_moves(body, tcx, tcx.param_env(def_id))
  {
    Ok((_, move_data)) => move_data,
    Err((move_data, _illegal_moves)) => {
      log::debug!("illegal moves found {_illegal_moves:?}");
      move_data
    }
  };
  let borrow_set =
    BorrowSet::build(tcx, body, locals_are_invalidated_at_exit, &move_data);
  let def_id = def_id.to_def_id();
  let param_env = tcx.param_env_reveal_all_normalized(def_id);

  let mut ctxt = PermissionsCtxt {
    tcx,
    permissions_output: Output::default(),
    polonius_input_facts,
    polonius_output,
    body_id,
    def_id,
    body_with_facts,
    borrow_set,
    move_data,
    param_env,
    loan_regions: None,
    place_data: IndexVec::new(),
    rev_lookup: HashMap::default(),
  };

  derive_permission_facts(&mut ctxt);

  ctxt.construct_loan_regions();

  log::info!(
    "permissions analysis for {:?} took: {:?}",
    name,
    timer.elapsed()
  );

  ctxt.borrow_set.location_map.iter().for_each(|(_k, bd)| {
    log::debug!("Borrow Data {:?}", bd);
  });

  ctxt
}
