//! Observe the installed reference parser in Vitest's worker, without changing its sources.
use neon::prelude::*;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

const MODULES: &[&str] = &[
    "parser",
    "index",
    "plaintext-renderer",
    "demo-content",
    "parser/block",
    "parser/inline",
    "parser/frontmatter",
    "parser/code-highlight",
];

// Copy enumerable data at call time. Only ancestor links are cycles: shared nodes
// appearing in two branches must still appear twice in the reference corpus.
fn snapshot<'a>(
    cx: &mut FunctionContext<'a>,
    value: Handle<'a, JsObject>,
) -> JsResult<'a, JsValue> {
    let parents = RefCell::new(Vec::<Root<JsObject>>::new());
    let replacer = JsFunction::new(cx, move |mut cx| {
        let value = cx.argument::<JsValue>(1)?;
        if let Ok(object) = value.downcast::<JsObject, _>(&mut cx) {
            let parent = cx.this_value();
            let mut stack = parents.borrow_mut();
            while stack
                .last()
                .is_some_and(|p| !p.to_inner(&mut cx).strict_equals(&mut cx, parent))
            {
                stack.pop();
            }
            if stack
                .iter()
                .any(|p| p.to_inner(&mut cx).strict_equals(&mut cx, object))
            {
                return Ok(cx.string("[Circular]").upcast());
            }
            stack.push(object.root(&mut cx));
        }
        Ok(value)
    })?;
    let json = cx.global::<JsObject>("JSON")?;
    let text: Handle<JsString> = json
        .method(cx, "stringify")?
        .args((value, replacer))?
        .call()?;
    json.method(cx, "parse")?.arg(text)?.call()
}

fn observation<'a>(
    cx: &mut FunctionContext<'a>,
    name: &str,
    first: Handle<'a, JsValue>,
    result: Handle<'a, JsValue>,
    parse: &Root<JsFunction>,
) -> JsResult<'a, JsValue> {
    let record = cx.empty_object();
    let op = cx.string(name);
    record.set(cx, "operation", op)?;
    if name == "highlightCodeBlock" {
        record.set(cx, "code", first)?;
        record.set(cx, "result", result)?;
    } else {
        let source = if name == "getMarkdownDemo" {
            result
        } else {
            first
        };
        if source.is_a::<JsString, _>(cx) {
            record.set(cx, "source", source)?;
            let doc = if name == "parse" {
                result
            } else {
                parse.to_inner(cx).bind(cx).arg(source)?.call()?
            }
            .downcast_or_throw::<JsObject, _>(cx)?;
            let ast: Handle<JsValue> = doc.get(cx, "ast")?;
            record.set(cx, "ast", ast)?;
        } else if let Ok(node) = first.downcast::<JsObject, _>(cx)
            && node
                .get::<JsValue, _, _>(cx, "type")?
                .is_a::<JsString, _>(cx)
        {
            record.set(cx, "ast", node)?;
        }
    }
    snapshot(cx, record)
}

fn install_spies(
    cx: &mut FunctionContext,
    current: &Rc<RefCell<Option<Root<JsArray>>>>,
) -> NeonResult<()> {
    let modules = cx.argument::<JsArray>(0)?;
    // Save the original parser before spying on its public re-exports.
    let parser: Handle<JsObject> = modules.get(cx, 0)?;
    let parse = Rc::new(parser.get::<JsFunction, _, _>(cx, "parse")?.root(cx));
    let inside = Rc::new(Cell::new(false));
    let vi = cx.global::<JsObject>("vi")?;
    for module in modules.to_vec(cx)? {
        let module = module.downcast_or_throw::<JsObject, _>(cx)?;
        for key in module.get_own_property_names(cx)?.to_vec(cx)? {
            let key = key.downcast_or_throw::<JsString, _>(cx)?;
            let value: Handle<JsValue> = module.get(cx, key)?;
            let Ok(original) = value.downcast::<JsFunction, _>(cx) else {
                continue;
            };
            if vi
                .method(cx, "isMockFunction")?
                .arg(original)?
                .call::<bool>()?
            {
                continue;
            }
            let original = original.root(cx);
            let name = key.value(cx);
            let (current, inside, parse) = (current.clone(), inside.clone(), parse.clone());
            let wrapper = JsFunction::new(cx, move |mut cx| {
                let args: Vec<_> = (0..cx.len()).filter_map(|i| cx.argument_opt(i)).collect();
                let original = original.to_inner(&mut cx);
                let receiver = cx.this_value();
                let was_inside = inside.replace(true);
                let observed = (|| {
                    let result = original.call(&mut cx, receiver, &args)?;
                    if !was_inside && let Some(calls) = current.borrow().as_ref() {
                        let calls = calls.to_inner(&mut cx);
                        let first = args
                            .first()
                            .copied()
                            .unwrap_or_else(|| cx.undefined().upcast());
                        let data = observation(&mut cx, &name, first, result, &parse)?;
                        let index = calls.len(&mut cx);
                        calls.set(&mut cx, index, data)?;
                    }
                    Ok(result)
                })();
                inside.set(was_inside);
                observed
            })?;
            let spy: Handle<JsFunction> = vi.method(cx, "spyOn")?.args((module, key))?.call()?;
            spy.method(cx, "mockImplementation")?.arg(wrapper)?.exec()?;
        }
    }
    Ok(())
}

#[neon::main]
fn main(mut cx: ModuleContext) -> NeonResult<()> {
    let current = Rc::new(RefCell::new(None));
    let slot = current.clone();
    let start = JsFunction::new(&mut cx, move |mut cx| {
        let context = cx.argument::<JsValue>(0)?;
        let task: Handle<JsObject> = cx
            .global::<JsObject>("Reflect")?
            .method(&mut cx, "get")?
            .args((context, "task"))?
            .call()?;
        let meta: Handle<JsObject> = task.get(&mut cx, "meta")?;
        let calls = cx.empty_array();
        meta.set(&mut cx, "calls", calls)?;
        slot.replace(Some(calls.root(&mut cx)));
        Ok(cx.undefined())
    })?;
    cx.global::<JsFunction>("beforeEach")?
        .bind(&mut cx)
        .arg(start)?
        .exec()?;
    let install = JsFunction::new(&mut cx, move |mut cx| {
        let vi = cx.global::<JsObject>("vi")?;
        let base: String = cx
            .global::<JsObject>("process")?
            .method(&mut cx, "cwd")?
            .call()?;
        let imports = cx.empty_array();
        for (i, module) in MODULES.iter().enumerate() {
            let path = format!(
                "{base}/upstream/packages/toolcraft-design/src/terminal-markdown/{module}.ts"
            );
            let promise: Handle<JsObject> =
                vi.method(&mut cx, "importActual")?.arg(path)?.call()?;
            imports.set(&mut cx, i as u32, promise)?;
        }
        let current = current.clone();
        let ready = JsFunction::new(&mut cx, move |mut cx| {
            install_spies(&mut cx, &current)?;
            Ok(cx.undefined())
        })?;
        let all: Handle<JsObject> = cx
            .global::<JsFunction>("Promise")?
            .method(&mut cx, "all")?
            .arg(imports)?
            .call()?;
        all.method(&mut cx, "then")?
            .arg(ready)?
            .call::<Handle<JsObject>>()
    })?;
    cx.global::<JsFunction>("beforeAll")?
        .bind(&mut cx)
        .arg(install)?
        .exec()?;
    Ok(())
}
