use std::collections::hash_map::Entry;

use flowistry::mir::utils::{PlaceExt as FlowistryPlaceExt, SpanExt};
use fluid_let::fluid_let;
use rustc_data_structures::fx::FxHashMap as HashMap;
use rustc_hir::{
  self as hir,
  intravisit::{self, Visitor as HirVisitor},
};
use rustc_middle::{hir::nested_filter, mir::Place};
use rustc_span::Span;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{
  analysis::{
    ir_mapper::{GatherDepth, GatherMode, IRMapper},
    permissions::{
      Permissions, PermissionsCtxt, PermissionsData, PermissionsDomain,
    },
    LoanKey,
  },
  errors,
  mir::utils::PlaceExt as AquascopePlaceExt,
  Range,
};

fluid_let!(pub static INCLUDE_MODE: PermIncludeMode);

#[derive(Debug, PartialEq, Eq, Clone, Copy, Deserialize, Serialize, Hash)]
pub enum PermIncludeMode {
  Changes,
  All,
}

impl std::str::FromStr for PermIncludeMode {
  type Err = String;
  fn from_str(s: &str) -> Result<Self, Self::Err> {
    match s {
      "Changes" => Ok(Self::Changes),
      "All" => Ok(Self::All),
      _ => Err(format!("Could not parse: {s}")),
    }
  }
}

pub trait Difference {
  type Diff;

  fn diff(&self, rhs: Self) -> Self::Diff;
}

#[derive(Clone, Debug, Serialize, TS)]
#[ts(export)]
pub struct PermissionsStateStep {
  pub location: Range,
  pub state: Vec<(String, PermissionsDataDiff)>,
}

#[derive(Clone, Debug, Serialize, TS)]
#[serde(tag = "type")]
#[ts(export)]
pub enum ValueStep<A>
where
  A: Clone + std::fmt::Debug + Serialize + TS,
{
  High,
  Low,
  None {
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<A>,
  },
}

// A handy macro for making difference types with only BoolStep fields
// TODO(gavinleroy): a diff type should be automatically generated if all the fields
// in a macro can ge diffed, but I'll save that for later. Below is mostly a syntactic
// macro to simplify things for the time being.
// FIXME: no longer sufficient *rewrite*. Shouldn't need to pass the name $diff
// and fields should be able to have a specified type, if not provided then
// the default BoolStep can be taken.
macro_rules! make_diff {
  ($base:ident => $diff:ident { $($i:ident),* }) => {
    #[derive(Clone, Debug, Serialize, TS)]
    #[ts(export)]
    pub struct $diff {
      $( pub $i: ValueStep<bool>, )*
    }

    impl $diff {
      fn is_empty(&self) -> bool {
        $( self.$i.is_empty() && )* true
      }
    }
  }
}

make_diff!(Permissions => PermissionsDiff {
   read, write, drop
});

#[derive(Clone, Debug, Serialize, TS)]
#[ts(export)]
pub struct PermissionsDataDiff {
  pub is_live: ValueStep<bool>,
  pub type_droppable: ValueStep<bool>,
  pub type_writeable: ValueStep<bool>,
  pub path_moved: ValueStep<bool>,
  pub loan_read_refined: ValueStep<LoanKey>,
  pub loan_write_refined: ValueStep<LoanKey>,
  pub loan_drop_refined: ValueStep<LoanKey>,
  pub permissions: PermissionsDiff,
}

impl PermissionsDataDiff {
  fn is_empty(&self) -> bool {
    self.permissions.is_empty()
  }
}

impl Difference for bool {
  type Diff = ValueStep<bool>;
  fn diff(&self, rhs: bool) -> Self::Diff {
    if *self && !rhs {
      ValueStep::Low
    } else if !*self && rhs {
      ValueStep::High
    } else {
      ValueStep::None { value: Some(*self) }
    }
  }
}

impl<T> ValueStep<T>
where
  T: Clone + std::fmt::Debug + Serialize + TS,
{
  fn is_empty(&self) -> bool {
    matches!(self, Self::None { .. })
  }
}

impl<A> Difference for Option<A>
where
  A: Clone + PartialEq + Eq + std::fmt::Debug + Serialize + TS,
{
  type Diff = ValueStep<A>;

  fn diff(&self, rhs: Option<A>) -> Self::Diff {
    match (self, rhs) {
      (None, None) => ValueStep::None { value: None },
      (Some(_), None) => ValueStep::Low,
      (None, Some(_)) => ValueStep::High,
      (Some(v0), Some(v1)) => {
        if *v0 != v1 {
          log::warn!(
            "Option diff Some does not contain same value {v0:?} -> {v1:?}"
          );
        }
        ValueStep::None { value: Some(v1) }
      }
    }
  }
}

impl Difference for Permissions {
  type Diff = PermissionsDiff;

  fn diff(&self, rhs: Permissions) -> Self::Diff {
    PermissionsDiff {
      read: self.read.diff(rhs.read),
      write: self.write.diff(rhs.write),
      drop: self.drop.diff(rhs.drop),
    }
  }
}

impl Difference for PermissionsData {
  type Diff = PermissionsDataDiff;

  fn diff(&self, rhs: PermissionsData) -> Self::Diff {
    PermissionsDataDiff {
      is_live: self.is_live.diff(rhs.is_live),
      type_droppable: self.type_droppable.diff(rhs.type_droppable),
      type_writeable: self.type_writeable.diff(rhs.type_writeable),
      loan_read_refined: self.loan_read_refined.diff(rhs.loan_read_refined),
      loan_write_refined: self.loan_write_refined.diff(rhs.loan_write_refined),
      loan_drop_refined: self.loan_drop_refined.diff(rhs.loan_drop_refined),
      path_moved: self.path_moved.diff(rhs.path_moved),
      permissions: self.permissions.diff(rhs.permissions),
    }
  }
}

impl<'tcx> Difference for &PermissionsDomain<'tcx> {
  type Diff = HashMap<Place<'tcx>, PermissionsDataDiff>;
  fn diff(&self, rhs: &PermissionsDomain<'tcx>) -> Self::Diff {
    self
      .iter()
      .fold(HashMap::default(), |mut acc, (place, p1)| {
        let p2 = rhs.get(place).unwrap();
        let diff = p1.diff(*p2);

        match acc.entry(*place) {
          Entry::Occupied(_) => {
            panic!("Permissions step already in output for {place:?}");
          }
          Entry::Vacant(entry) => {
            entry.insert(diff);
          }
        }

        acc
      })
  }
}

// -----------------------------------------------

pub fn compute_permission_steps<'a, 'tcx>(
  ctxt: &PermissionsCtxt<'a, 'tcx>,
  span_to_range: impl Fn(Span) -> Range,
) -> Vec<PermissionsStateStep>
where
  'tcx: 'a,
{
  let tcx = ctxt.tcx;
  let body = &ctxt.body_with_facts.body;
  let _basic_blocks = body.basic_blocks.indices();

  let ir_mapper = &IRMapper::new(tcx, body, GatherMode::IgnoreCleanup);

  let mode = INCLUDE_MODE.copied().unwrap_or(PermIncludeMode::Changes);

  let mut hir_visitor = HirPermDiffFlow {
    ctxt,
    ir_mapper,
    mode,
    diff: HashMap::default(),
    step_barriers: Vec::default(),
    visibility_scopes: Vec::default(),
  };

  hir_visitor.visit_nested_body(ctxt.body_id);

  prettify_permission_steps(ctxt, hir_visitor.diff, span_to_range)
}

// Prettify, means:
// - Remove all places that are not source visible
// - Remove all tables which are empty
// - Sanitize spans (mostly for macro invocation)
// - Convert Spans to Ranges
fn prettify_permission_steps<'tcx>(
  ctxt: &PermissionsCtxt<'_, 'tcx>,
  perm_steps: HashMap<Span, HashMap<Place<'tcx>, PermissionsDataDiff>>,
  span_to_range: impl Fn(Span) -> Range,
) -> Vec<PermissionsStateStep> {
  let tcx = ctxt.tcx;
  let body = &ctxt.body_with_facts.body;

  macro_rules! place_to_string {
    ($p:expr) => {
      $p.to_string(tcx, body)
        .unwrap_or_else(|| String::from("<var>"))
    };
  }

  let first_error_span_opt =
    errors::get_span_of_first_error(ctxt.def_id.expect_local())
      .and_then(|s| s.as_local(ctxt.body_with_facts.body.span));
  let source_map = tcx.sess.source_map();

  perm_steps
    .into_iter()
    .fold(
      HashMap::<Span, Vec<(Place<'tcx>, PermissionsDataDiff)>>::default(),
      |mut acc, (unsanitized_span, place_to_diffs)| {
        let span = unsanitized_span
          .as_local(ctxt.body_with_facts.body.span)
          .unwrap_or(unsanitized_span);

        // Attach the span to the end of the line. Later, all permission
        // steps appearing on the same line will be combined.
        let span = source_map.span_extend_to_line(span).shrink_to_hi();

        let entries = place_to_diffs
          .into_iter()
          .filter(|(place, _)| place.is_source_visible(tcx, body))
          .collect::<Vec<_>>();

        // This could be a little more graceful. The idea is that
        // we want to remove all permission steps which occur after
        // the first error, but the steps involved with the first
        // error could still be helpful. This is why we filter all
        // spans with a LO BytePos greater than the error
        // span HI BytePos.
        if !(entries.is_empty()
          || first_error_span_opt
            .is_some_and(|err_span| err_span.hi() < span.lo()))
        {
          acc.entry(span).or_default().extend(entries);
        }

        acc
      },
    )
    .into_iter()
    .map(|(span, mut entries)| {
      let range = span_to_range(span);

      entries
        .sort_by_key(|(place, _)| (place.local.as_usize(), place.projection));

      let state = entries
        .into_iter()
        .map(|(place, diff)| {
          let s = place_to_string!(place);
          (s, diff)
        })
        .collect::<Vec<_>>();

      PermissionsStateStep {
        location: range,
        state,
      }
    })
    .collect::<Vec<_>>()
}

struct HirPermDiffFlow<'a, 'tcx>
where
  'tcx: 'a,
{
  ctxt: &'a PermissionsCtxt<'a, 'tcx>,
  ir_mapper: &'a IRMapper<'a, 'tcx>,
  mode: PermIncludeMode,
  // For a given Span, we attach the before and after Domain.
  // The difference of the domains is not computed yet because we may have
  // to combine differences for processing at the source level.
  //
  // NOTE: this post-processing is needed because the current version
  // will only show one permissions diff per line (mostly for aesthetic reasons).
  // A future iteration of this may decide to remove this restriction.
  diff: HashMap<Span, HashMap<Place<'tcx>, PermissionsDataDiff>>,
  step_barriers: Vec<Place<'tcx>>,
  visibility_scopes: Vec<HashMap<Place<'tcx>, PermissionsDataDiff>>,
}

impl<'tcx> HirPermDiffFlow<'_, 'tcx> {
  fn domain_bottom(&self) -> PermissionsDomain<'tcx> {
    self
      .ctxt
      .domain_places()
      .into_iter()
      .map(|place| {
        (place, PermissionsData {
          is_live: false,
          type_droppable: false,
          type_writeable: false,
          type_copyable: false,
          path_moved: false,
          loan_read_refined: None,
          loan_write_refined: None,
          loan_drop_refined: None,
          permissions: Permissions::bottom(),
        })
      })
      .collect::<HashMap<_, _>>()
      .into()
  }

  fn should_keep(&self, p: &PermissionsDataDiff) -> bool {
    !(matches!(p.is_live, ValueStep::None { value: Some(false) })
      || (self.mode == PermIncludeMode::Changes && p.is_empty()))
  }
}

macro_rules! filter_exec_commit {
  ($this:tt, $id:expr, $scopes:ident, $inner:expr) => {
    // For the current visibility we remove any place which is part of a barrier,
    // or if that place had *no change* (e.g. an empty difference).
    $scopes.retain(|place, df| {
      $this.should_keep(&df)
        && !$this.step_barriers.iter().any(|p| p.local == place.local)
    });

    // For each previous visibility scope, remove the permission differences
    // that are visible at the current level. This ensures that the permission
    // change is attached to the most nested HirId possible.
    for scope in $this.visibility_scopes.iter_mut() {
      scope.retain(|place, _df| {
        // TODO: is it sufficient to just compare the place?
        // Try and come up with a counter example or a proof for why this is [un]safe.
        !$scopes.contains_key(place)
      });
    }
    $this.visibility_scopes.push($scopes);
    // Execute the inner block
    $inner;
    let $scopes = $this.visibility_scopes.pop().unwrap();
    let span = $this.ctxt.tcx.hir().span($id);

    // FIXME: for desugaring and macro expansion, it's possible for multiple
    // permission step boxes to hash to the exact same span. This is
    // not currently handled and will creep up on us most definitely.
    $this.diff.entry(span).or_default().extend($scopes);
  }
}

macro_rules! in_visibility_scope_context {
  ($this:tt, $id:tt, $inner:block) => {
    // From the beginning to the point *after* the end location of `id`, `visibility_here`
    // represents all the places where a permissions change was visible. Specifically,
    // this is the difference in PermissionsDomain @ (after_point - before_point).
    let mut visibility_here =
      $this.ir_mapper.get_mir_locations($id, GatherDepth::Nested).and_then(|mir_order| {
        mir_order.get_entry_exit_locations().map(|(loc1, loc2)| {
          let before_point = $this.ctxt.location_to_point(loc1);
          let after_point = $this.ctxt.location_to_point(loc2);
          let dmn_before = &$this.ctxt.permissions_domain_at_point(before_point);
          let dmn_after = &$this
            .ctxt
            .permissions_domain_after_point_effect(after_point)
            .unwrap_or_else(|| $this.ctxt.permissions_domain_at_point(after_point));
          dmn_before.diff(dmn_after)
        })
      }).unwrap_or_else(|| HashMap::default());
    filter_exec_commit!($this, $id, visibility_here, $inner);
  };
}

macro_rules! with_seamless_branching {
  ($this:tt, $k:ident, $targets:expr, $branch_cnd:expr) => {
    let id = $branch_cnd.hir_id;
    let hir = $this.ctxt.tcx.hir();
    log::debug!("Visiting EXPR CND: {}",  hir.node_to_string(id));
    // From the beginning to the point *after* the end location of `id`, `visibility_here`
    // represents all the places where a permissions change was visible. Specifically,
    // this is the difference in PermissionsDomain @ (after_point - before_point).
    let (mut visibility_here, after_branch_point_opt) =
      $this.ir_mapper.get_mir_locations(id, GatherDepth::Nested).and_then(|mir_order| {
        mir_order.get_entry_exit_locations().map(|(loc1, loc2)| {
          let before_point = $this.ctxt.location_to_point(loc1);
          let after_point = $this.ctxt.location_to_point(loc2);
          let dmn_before = &$this.ctxt.permissions_domain_at_point(before_point);
          let dmn_after = &$this.ctxt.permissions_domain_at_point(after_point);
          (dmn_before.diff(dmn_after), Some(after_point))
        })
      }).unwrap_or_else(|| (HashMap::default(), None));
    filter_exec_commit!($this, id, visibility_here, {
      intravisit::walk_expr($this, $branch_cnd);
    });
    // - for each landing pad:
    // --- diff the domain starting from the conditions "after point"
    // --- rinse and repeat
    for target in $targets.iter() {
      let id = target.hir_id;
      // From the beginning to the point *after* the end location of `id`, `visibility_here`
      // represents all the places where a permissions change was visible. Specifically,
      // this is the difference in PermissionsDomain @ (after_point - before_point).
      if let Some(mut visibility_here) =
        $this.ir_mapper.get_mir_locations(id, GatherDepth::Nested).and_then(|mir_order| {
          mir_order.get_entry_exit_locations().map(|(loc1, loc2)| {
            let before_point = after_branch_point_opt.unwrap_or_else(|| {
              $this.ctxt.location_to_point(loc1)
            });
            let after_point = $this.ctxt.location_to_point(loc2);
            let dmn_before = &$this.ctxt.permissions_domain_at_point(before_point);
            let dmn_after = &$this
              .ctxt
              .permissions_domain_after_point_effect(after_point)
              .unwrap_or_else(|| $this.ctxt.permissions_domain_at_point(after_point));
            dmn_before.diff(dmn_after)
          })
        }) {
          filter_exec_commit!($this, id, visibility_here, {
            intravisit::$k($this, target);
          });
        } else {
          log::warn!("No target location found for branch landing pad {}", hir.node_to_string(id));
        }
    }
  }
}

// NOTE: in the following traversal a HIR Node (N) which should be wrapped in a
// visibility context, e.g. one which can have permission changes stuck to
// the end of it, will be denoted with the following notation: `??? N ???`
//
// That is for an initial permissions domain ?? and difference set S,
// the execution of N will produce some context ??' where ??' - ?? = S.
// Nested contexts `??? ??? N ???: S_1 ???: S_0` ensure that S_0 ??? S_1 = ??? .
// Meaning that permission changes are sucked into the most nested context.
//
// Additionally, there is a barrier context ??, which forbids moving a `Variable`'s
// permission changes into a nested context.
// In the following statement we ensure that S_1 doesnot include
// any permission changes for the variable `s`.
// ```
// ??; ?? ??? {s}  :- ??? E ???: S_1 => ??';??
// --------------------------------------
// ??;?? :-  ??? let s = E; ??? : S_0 => ??';??
// ```
impl<'a, 'tcx: 'a> HirVisitor<'tcx> for HirPermDiffFlow<'a, 'tcx> {
  type NestedFilter = nested_filter::All;

  fn nested_visit_map(&mut self) -> Self::Map {
    self.ctxt.tcx.hir()
  }

  fn visit_body(&mut self, body: &'tcx hir::Body) {
    let body_id = body.value.hir_id;
    let empty_domain = &self.domain_bottom();
    self
      .ir_mapper
      .get_mir_locations(body_id, GatherDepth::Nested)
      .iter()
      .for_each(|mir_order| {
        mir_order.entry_location().iter().for_each(|entry_loc| {
          let entry_point = self.ctxt.location_to_point(*entry_loc);
          let entry_domain = self.ctxt.permissions_domain_at_point(entry_point);
          let d = empty_domain
            .diff(&entry_domain)
            .into_iter()
            .filter(|(_, df)| self.should_keep(&df))
            .collect::<HashMap<_, _>>();
          let id = body.id().hir_id;
          let span = self.ctxt.tcx.hir().span(id).shrink_to_lo();
          self.diff.insert(span, d);
        })
      });

    intravisit::walk_body(self, body);
  }

  // Statements:
  // Can have 4 different kinds: Local, Item, Expr, Semi.
  //
  // For all of these different variants, we want to attach a
  // visibility scope to the outside of the statement. This ensures,
  // that any statement that has a change in permissions will show
  // this as a step at the statement level.
  //
  // ```
  // ??? <stmt> ???
  // ```
  fn visit_stmt(&mut self, stmt: &'tcx hir::Stmt) {
    let id = stmt.hir_id;
    let hir = self.ctxt.tcx.hir();

    log::debug!("Visiting STMT: {}", hir.node_to_string(id));

    in_visibility_scope_context!(self, id, {
      intravisit::walk_stmt(self, stmt);
    });
  }

  // Blocks:
  // ```
  // ??? { s_1, s_2, ... s_n, ??? expr? ??? } ???
  // ```
  fn visit_block(&mut self, block: &'tcx hir::Block) {
    let id = block.hir_id;
    let hir = self.ctxt.tcx.hir();

    log::debug!("Visiting BLOCK: {}", hir.node_to_string(id));

    in_visibility_scope_context!(self, id, {
      for stmt in block.stmts.iter() {
        self.visit_stmt(stmt);
      }

      if let Some(expr) = block.expr {
        let id = expr.hir_id;
        in_visibility_scope_context!(self, id, {
          self.visit_expr(expr);
        });
      }
    });
  }

  // A Local of the form:
  //
  // ```
  // let symbol = ?? ??? {symbol} expr else { ??? block ??? };
  // ```
  fn visit_local(&mut self, local: &'tcx hir::Local) {
    let id = local.hir_id;
    let hir = self.ctxt.tcx.hir();

    log::debug!("Visiting LOCAL: {}", hir.node_to_string(id));

    // NOTE: We add a "step barrier" for local assignments to make sure the permissions
    // for an assigned local don't come alive too early. Consider the following:
    // ```
    // let x = if <cnd> {
    //   <expr:1>
    // } else {
    //   <expr:2>
    // }
    // ```
    // Due to how the MIR is generated, the MIR statement assigning to `x`,
    // happens at the end of the block <expr:1>. This means, that if we don't
    // pull those permissions out to the assignment, they will only occur once
    // (specifically inside the "then branch") for the whole let.
    let mut added_barrier = false;

    if let Some(place) = self.ir_mapper.local_assigned_place(local) {
      self.step_barriers.push(place);
      added_barrier = true;
    }

    let pre_visibility_ctxt = self.visibility_scopes.clone();

    if let Some(expr) = local.init {
      self.visit_expr(expr);
    }

    if let Some(_block) = local.els {
      // TODO:
      self.visibility_scopes = pre_visibility_ctxt;
      unimplemented!("Locals with else branch");
    }

    if added_barrier {
      self.step_barriers.pop();
    }
  }

  fn visit_expr(&mut self, expr: &'tcx hir::Expr) {
    use hir::ExprKind as EK;
    let id = expr.hir_id;
    let hir = self.ctxt.tcx.hir();
    log::debug!("Visiting EXPR: {}", hir.node_to_string(id));

    match expr.kind {
      // ```
      // if ??? cnd ???  {
      //   ??? e_then ???
      // } else {
      //   ??? e_else? ???
      // }
      // ```
      EK::If(cnd, then, else_opt) => {
        in_visibility_scope_context!(self, id, {
          let landing_pads = [Some(then), else_opt]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
          with_seamless_branching!(self, walk_expr, landing_pads, cnd);
        });
      }

      EK::Match(swtch, arms, _source) => {
        in_visibility_scope_context!(self, id, {
          with_seamless_branching!(self, walk_arm, arms, swtch);
        });
      }

      // - Variants I'd need to think more about.
      // EK::Closure(..) => unimplemented!("CLOSURES"),
      _ => {
        intravisit::walk_expr(self, expr);
      }
    }
  }
}
