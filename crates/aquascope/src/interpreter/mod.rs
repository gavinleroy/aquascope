//! Rust runtime visualizer using Miri

use anyhow::Result;
use either::Either;
use flowistry::mir::utils::SpanExt;
use rustc_middle::ty::TyCtxt;

mod mapper;
mod miri_utils;
mod mvalue;
mod step;

pub use mvalue::MValue;
pub use step::MStep;

use crate::{interpreter::mapper::Mapper, Range};

pub(crate) fn interpret(tcx: TyCtxt) -> Result<Vec<MStep<Range>>> {
  let mut evaluator = step::VisEvaluator::new(tcx).unwrap();
  let mir_steps = evaluator.eval().map_err(|e| {
    e.print_backtrace();
    anyhow::format_err!("{}", e)
  })?;

  // eprintln!("{mir_steps:#?}");

  let mapper = Mapper::new(&evaluator.ecx);
  let hir_steps =
    mapper::group_steps(mir_steps, |loc| mapper.abstract_loc(loc));

  // for step in &hir_steps {
  //   let (_, hir_body_loc) = step.stack.frames.last().unwrap().location;
  //   eprintln!(
  //     "{:?}",
  //     match hir_body_loc {
  //       Either::Left(node_id) => tcx.hir().node_to_string(node_id),
  //       Either::Right(span) => tcx.sess.source_map().span_to_snippet(span).unwrap(),
  //     }
  //   );
  // }
  // eprintln!("{hir_steps:#?}");

  let src_steps = mapper::group_steps(hir_steps, |(owner_id, hir_body_loc)| {
    let hir = tcx.hir();
    let outer_span = hir.span_with_body(owner_id);
    let span = match hir_body_loc {
      Either::Left(node_id) => hir.span(node_id).as_local(outer_span)?,
      Either::Right(span) => span.as_local(outer_span)?,
    };
    let range =
      flowistry::source_map::Range::from_span(span, tcx.sess.source_map())
        .unwrap();
    Some(Range::from(range))
  });

  Ok(src_steps)
}

#[derive(Default)]
pub struct InterpretCallbacks {
  pub result: Option<Result<Vec<MStep<Range>>>>,
}

impl rustc_driver::Callbacks for InterpretCallbacks {
  fn after_parsing<'tcx>(
    &mut self,
    _compiler: &rustc_interface::interface::Compiler,
    queries: &'tcx rustc_interface::Queries<'tcx>,
  ) -> rustc_driver::Compilation {
    queries.global_ctxt().unwrap().take().enter(|tcx| {
      self.result = Some(interpret(tcx));
    });
    rustc_driver::Compilation::Stop
  }
}
