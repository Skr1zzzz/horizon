//! The buffers a pass may read past their first mip level (6.1.7), so that a mip chain no pass
//! reads below its first level isn't made: such a chain changes no pixel.
//!
//! A read stays on the first level when the shader names level 0: `texelFetch(t, p, 0)`,
//! `textureLod(t, p, 0.0)`, `textureSize(t, 0)`, a gather. Any other sample (an implicit or
//! biased level, gradients, a level worked out at run time) may reach the levels below it, and
//! `textureQueryLevels` counts them. A texture handed to a function counts with what the
//! function reads of it.

use naga::{Expression, Handle};

use super::{Binding, Source};

/// What a function may read past the first level: of the module's textures, and of its own
/// texture arguments (bit n: argument n).
#[derive(Default)]
struct Deep {
    globals: Vec<Handle<naga::GlobalVariable>>,
    arguments: u64,
    /// A read of a texture this doesn't follow, which may then be any.
    unknown: bool,
}

impl Deep {
    /// Adds the texture `image`, an expression of `function`.
    fn add(&mut self, function: &naga::Function, image: Handle<Expression>) {
        match function.expressions[image] {
            Expression::GlobalVariable(g) => {
                if !self.globals.contains(&g) {
                    self.globals.push(g);
                }
            }
            Expression::FunctionArgument(n) if n < 64 => self.arguments |= 1 << n,
            _ => self.unknown = true,
        }
    }
}

/// Whether expression `expr` of `expressions` (a function's, or `module`'s constant ones) is a
/// constant zero.
fn zero(
    module: &naga::Module,
    expressions: &naga::Arena<Expression>,
    expr: Handle<Expression>,
) -> bool {
    use std::num::FpCategory::Zero;
    match expressions[expr] {
        Expression::Literal(literal) => match literal {
            naga::Literal::F64(v) | naga::Literal::AbstractFloat(v) => v.classify() == Zero,
            naga::Literal::F32(v) => v.classify() == Zero,
            naga::Literal::U32(v) => v == 0,
            naga::Literal::I32(v) => v == 0,
            naga::Literal::U64(v) => v == 0,
            naga::Literal::I64(v) | naga::Literal::AbstractInt(v) => v == 0,
            _ => false,
        },
        Expression::ZeroValue(_) => true,
        Expression::Constant(c) => {
            zero(module, &module.global_expressions, module.constants[c].init)
        }
        // A conversion of zero is zero (a bitcast of -0.0 is not).
        Expression::As {
            expr,
            convert: Some(_),
            ..
        } => zero(module, expressions, expr),
        _ => false,
    }
}

/// Calls `f` with each function `block` calls and the call's arguments, in nested blocks too.
fn calls(block: &naga::Block, f: &mut impl FnMut(Handle<naga::Function>, &[Handle<Expression>])) {
    for statement in block.iter() {
        match statement {
            naga::Statement::Block(b) => calls(b, f),
            naga::Statement::If { accept, reject, .. } => {
                calls(accept, f);
                calls(reject, f);
            }
            naga::Statement::Switch { cases, .. } => {
                for case in cases {
                    calls(&case.body, f);
                }
            }
            naga::Statement::Loop {
                body, continuing, ..
            } => {
                calls(body, f);
                calls(continuing, f);
            }
            naga::Statement::Call {
                function,
                arguments,
                ..
            } => f(*function, arguments),
            _ => {}
        }
    }
}

/// What `function` may read past the first level, given `callees`: what each function before it
/// in `module` does (naga calls only functions declared earlier; a later one is unknown).
fn deep_of(module: &naga::Module, function: &naga::Function, callees: &[Deep]) -> Deep {
    let mut deep = Deep::default();
    let first = |level| zero(module, &function.expressions, level);
    for (_, e) in function.expressions.iter() {
        let image = match *e {
            Expression::ImageSample { image, level, .. } => match level {
                naga::SampleLevel::Zero => None,
                naga::SampleLevel::Exact(l) if first(l) => None,
                _ => Some(image),
            },
            Expression::ImageLoad {
                image,
                level: Some(l),
                ..
            } if !first(l) => Some(image),
            Expression::ImageQuery { image, query } => match query {
                naga::ImageQuery::NumLevels => Some(image),
                naga::ImageQuery::Size { level: Some(l) } if !first(l) => Some(image),
                _ => None,
            },
            _ => None,
        };
        if let Some(image) = image {
            deep.add(function, image);
        }
    }
    calls(&function.body, &mut |callee, arguments| {
        let Some(c) = callees.get(callee.index()) else {
            deep.unknown = true;
            return;
        };
        deep.unknown |= c.unknown;
        for &g in &c.globals {
            if !deep.globals.contains(&g) {
                deep.globals.push(g);
            }
        }
        for n in (0..64).filter(|n| c.arguments & 1 << n != 0) {
            match arguments.get(n) {
                Some(&a) => deep.add(function, a),
                None => deep.unknown = true,
            }
        }
    });
    deep
}

/// The set 0 bindings of the textures `module`'s entry points may read past their first level,
/// or count the levels of; `None` when that can't be told.
pub(super) fn deep_bindings(module: &naga::Module) -> Option<Vec<u32>> {
    let mut callees = Vec::with_capacity(module.functions.len());
    for (_, function) in module.functions.iter() {
        let deep = deep_of(module, function, &callees);
        callees.push(deep);
    }
    let mut bindings = Vec::new();
    for entry in &module.entry_points {
        let deep = deep_of(module, &entry.function, &callees);
        if deep.unknown || deep.arguments != 0 {
            return None;
        }
        for g in deep.globals {
            match &module.global_variables[g].binding {
                Some(b) if b.group == 0 => bindings.push(b.binding),
                // Not a sampler of the pass's (those are all in set 0).
                Some(_) => {}
                None => return None,
            }
        }
    }
    Some(bindings)
}

/// The buffers among `samplers` that `modules` (a pass's stages) may read past their first
/// level: all of them when a module's reads can't be told.
pub(super) fn deep_buffers<'m>(
    samplers: &[Binding],
    modules: impl IntoIterator<Item = &'m naga::Module>,
) -> u16 {
    let mut deep = 0;
    for module in modules {
        let bindings = deep_bindings(module);
        for s in samplers {
            if let Source::Color(b) = s.source
                && bindings.as_ref().is_none_or(|d| d.contains(&s.binding))
            {
                deep |= 1 << b;
            }
        }
    }
    deep
}

/// Of the buffers `stale` whose mip chains a pass asks for, those to make: the ones some pass
/// may read past the first level while the chain stays current. `binders` are, in order from
/// the asking pass, what binds buffers until the chain is next made or the frame ends: each
/// one's deep reads (`deep_buffers`) and the buffers it writes, which end a chain's turn after
/// its own read. With `open`, more may bind after `binders` (the game's draws between the
/// pack's passes): a chain still current at their end is made.
pub(super) fn chains_to_make(
    stale: u16,
    binders: impl IntoIterator<Item = (u16, u16)>,
    open: bool,
) -> u16 {
    let (mut current, mut make) = (stale, 0);
    for (deep, written) in binders {
        make |= current & deep;
        current &= !(deep | written);
        if current == 0 {
            break;
        }
    }
    if open { make | current } else { make }
}
