// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! DOM host-surface tests. Engine-generic bodies (`*_works<E>`) instantiated
//! per backend (`*_on_boa` / `*_on_nova`); kept in one file for auditability.

use super::*;
use crate::Runtime;

/// JS builds and mutates a tree through `document`, exercised against any backend:
/// `createElement`/`createTextNode` mint nodes, `appendChild` parents them,
/// `setAttribute` + `textContent` mutate, and `getElementById` finds by id — all
/// landing in the host `ScriptedDom`, with the changes recorded as mutations.
fn dom_construction_works<E: ScriptEngine>() {
    let mut rt = Runtime::<E>::new().expect("runtime");

    rt.eval(
        "var d = document.createElement('div');\
         d.setAttribute('id', 'main');\
         var t = document.createTextNode('hello');\
         d.appendChild(t);\
         document.appendChild(d);\
         var found = document.getElementById('main');\
         found.textContent = 'world';",
    )
    .expect("dom script");

    {
        let host = rt.host().borrow();
        let dom = &host.dom;

        // The document root has the one <div> we appended.
        let root = dom.document();
        let kids: Vec<_> = dom.dom_children(root).collect();
        assert_eq!(kids.len(), 1, "div appended under document");
        let div = kids[0];

        // The div: a <div> element, id=main, textContent set to 'world'.
        assert_eq!(dom.element_name(div).unwrap().local, LocalName::from("div"));
        assert_eq!(
            dom.attribute(div, &Namespace::from(""), &LocalName::from("id")),
            Some("main"),
            "getElementById found the div and setAttribute stuck",
        );
        assert_eq!(dom.text(div), Some("world"), "textContent setter ran");

        // Its text-node child still carries the original data.
        let div_kids: Vec<_> = dom.dom_children(div).collect();
        assert_eq!(div_kids.len(), 1);
        assert_eq!(dom.text(div_kids[0]), Some("hello"));
    }

    // The structural + attribute + character-data changes were recorded for
    // genet-layout: setAttribute, two appendChilds, textContent → 4 mutations.
    // (createElement / createTextNode record nothing until parented.)
    let mut muts = Vec::new();
    rt.host().borrow_mut().dom.drain_mutations(&mut muts);
    assert_eq!(muts.len(), 4, "one attr + two inserts + one char-data");
}

/// The read surface, exercised against any backend: `getAttribute` / `tagName` /
/// `textContent` getter return strings, and a miss returns `null`
/// (`getAttribute` on an absent attr, `getElementById` with no match).
fn dom_read_surface_works<E: ScriptEngine>() {
    let mut rt = Runtime::<E>::new().expect("runtime");

    rt.eval(
        "var d = document.createElement('div');\
         d.setAttribute('id', 'main');\
         d.textContent = 'hello';\
         document.appendChild(d);\
         var el = document.getElementById('main');\
         console.log(el.getAttribute('id'));\
         console.log(el.tagName);\
         console.log(el.textContent);\
         console.log(String(el.getAttribute('nope')));\
         console.log(String(document.getElementById('nope')));",
    )
    .expect("read script");

    assert_eq!(
        rt.host().borrow().console,
        vec!["main", "DIV", "hello", "null", "null"],
    );
}

/// Reflector identity, exercised against any backend: two lookups of the same
/// node are `===` (canonical reflector + wrapper cache), distinct nodes are not,
/// and `document` is stable.
fn dom_identity_works<E: ScriptEngine>() {
    let mut rt = Runtime::<E>::new().expect("runtime");

    rt.eval(
        "var d = document.createElement('div');\
         d.setAttribute('id', 'main');\
         document.appendChild(d);\
         console.log(String(document.getElementById('main') === document.getElementById('main')));\
         console.log(String(document.getElementById('main') === d));\
         console.log(String(document.createElement('div') === document.createElement('div')));\
         console.log(String(document === document));",
    )
    .expect("identity script");

    // same node: ===; created === found-by-id; two fresh elements: not ===; doc stable.
    assert_eq!(
        rt.host().borrow().console,
        vec!["true", "true", "false", "true"]
    );
}

/// Prototype dispatch, exercised against any backend: methods live on
/// `Node.prototype` (shared, not per-object closures), `instanceof` works, the
/// `Document : Node` chain holds, and `parentNode` walks the real tree.
fn dom_prototype_dispatch_works<E: ScriptEngine>() {
    let mut rt = Runtime::<E>::new().expect("runtime");

    rt.eval(
        "var d = document.createElement('div');\
         var e = document.createElement('span');\
         document.appendChild(d);\
         console.log(String(d instanceof Node));\
         console.log(String(document instanceof Document));\
         console.log(String(document instanceof Node));\
         console.log(String(d.appendChild === e.appendChild));\
         console.log(String(d.parentNode === document));",
    )
    .expect("prototype script");

    // element is a Node; document is a Document and a Node; the method is shared
    // (same prototype function); parentNode walks back to the document.
    assert_eq!(
        rt.host().borrow().console,
        vec!["true", "true", "true", "true", "true"]
    );
}

/// `load_dom`, against any backend: a parsed source document becomes the live
/// DOM, so script sees `document.body`, `getElementById`, and tag queries over
/// the pre-existing tree.
fn load_dom_works<E: ScriptEngine>() {
    use genet_static_dom::StaticDocument;
    let mut rt = Runtime::<E>::new().expect("runtime");
    let src = StaticDocument::parse(
        "<html><head></head><body><div id='main'><p>hi</p></div></body></html>",
    );
    rt.load_dom(&src);

    rt.eval(
        "console.log(document.body ? document.body.tagName : 'no-body');\
         console.log(document.documentElement ? document.documentElement.tagName : 'no-root');\
         var m = document.getElementById('main');\
         console.log(m ? m.tagName : 'not-found');\
         console.log(String(main === m));\
         console.log(String(document.getElementsByTagName('p').length));",
    )
    .expect("query script");

    assert_eq!(
        rt.host().borrow().console,
        vec!["BODY", "HTML", "DIV", "true", "1"]
    );
}

/// Regression probe for the dom/events corpus: a self-closing
/// `<input id=target type=hidden/>` in the loaded body must be queryable by
/// `getElementById` and usable as an EventTarget (the shape of
/// `Event-defaultPrevented-after-dispatch`, which failed with "cannot convert
/// null to object" when the input wasn't found).
fn load_dom_finds_self_closing_input<E: ScriptEngine>() {
    use genet_static_dom::StaticDocument;
    let mut rt = Runtime::<E>::new().expect("runtime");
    let src = StaticDocument::parse(
        "<html><body><div id=log></div><input id=\"target\" type=\"hidden\" value=\"\"/></body></html>",
    );
    rt.load_dom(&src);
    rt.eval(
        "var t = document.getElementById('target');\
         console.log(t ? t.tagName : 'NULL');\
         if (t) {\
           t.addEventListener('foo', function(e){ e.preventDefault(); });\
           var ev = document.createEvent('Event');\
           ev.initEvent('foo', true, true);\
           t.dispatchEvent(ev);\
           console.log('prevented:' + ev.defaultPrevented);\
           console.log('src:' + (ev.srcElement === t));\
         }",
    )
    .expect("input query script");
    assert_eq!(
        rt.host().borrow().console,
        vec!["INPUT", "prevented:true", "src:true"]
    );
}

/// Probe for the Event-dispatch-bubble-canceled shape: the test builds
/// `[window, document, html, body, table, tbody, tr, td]` and calls
/// addEventListener on each — failing with "cannot convert null to object"
/// if any is null. Pins which of window / documentElement / body / a
/// table-descendant is missing.
fn event_target_chain_resolves<E: ScriptEngine>() {
    use genet_static_dom::StaticDocument;
    let mut rt = Runtime::<E>::new().expect("runtime");
    rt.load_dom(&StaticDocument::parse(
        "<html><body><table id=t style=\"display:none\"><tbody id=tb>\
         <tr id=r><td id=td>x</td></tr></tbody></table></body></html>",
    ));
    rt.eval(
        "function tag(n){ return n ? n.tagName : 'NULL'; }\
         console.log('win:' + (window ? 'ok' : 'NULL'));\
         console.log('doc:' + (document ? 'ok' : 'NULL'));\
         console.log('html:' + tag(document.documentElement));\
         console.log('body:' + tag(document.body));\
         console.log('table:' + tag(document.getElementById('t')));\
         console.log('tbody:' + tag(document.getElementById('tb')));\
         console.log('tr:' + tag(document.getElementById('r')));\
         console.log('td:' + tag(document.getElementById('td')));",
    )
    .expect("chain probe");
    // Whatever resolves vs NULL tells us the gap; assert the ones we expect to
    // work today and surface the rest.
    let log = rt.host().borrow().console.clone();
    assert_eq!(log[0], "win:ok", "window is an EventTarget");
    assert_eq!(log[1], "doc:ok");
    assert_eq!(log[2], "html:HTML", "documentElement resolves");
    assert_eq!(log[3], "body:BODY", "body resolves");
    assert_eq!(log[4], "table:TABLE", "table resolves (display:none kept)");
    assert_eq!(log[5], "tbody:TBODY", "tbody resolves");
    assert_eq!(log[6], "tr:TR", "tr resolves");
    assert_eq!(log[7], "td:TD", "td resolves");
}

#[test]
fn event_target_chain_resolves_on_boa() {
    event_target_chain_resolves::<script_engine_boa::BoaEngine>();
}

/// Minimal repro of Event-dispatch-bubble-canceled: register a listener on
/// window + a target via the bool-capture form, set cancelBubble before
/// dispatch, and confirm propagation is halted (window's listener does not
/// fire) without throwing.
fn cancel_bubble_before_dispatch<E: ScriptEngine>() {
    use genet_static_dom::StaticDocument;
    let mut rt = Runtime::<E>::new().expect("runtime");
    rt.load_dom(&StaticDocument::parse(
        "<html><body><div id=d>x</div></body></html>",
    ));
    rt.eval(
        "var fired = [];\
         var d = document.getElementById('d');\
         function h(e){ fired.push(e.currentTarget === window ? 'window' : 'node'); }\
         window.addEventListener('foo', h, true);\
         window.addEventListener('foo', h, false);\
         d.addEventListener('foo', h, true);\
         d.addEventListener('foo', h, false);\
         var evt = document.createEvent('Event');\
         evt.initEvent('foo', true, true);\
         evt.cancelBubble = true;\
         d.dispatchEvent(evt);\
         console.log('fired:' + fired.join(','));",
    )
    .expect("cancel-bubble repro");
    // cancelBubble=true before dispatch stops propagation: the capture walk
    // from window down is halted, so at most the target's own listeners run
    // (the spec stops *after* the current target). Key point: no throw, and
    // window (an ancestor) is not reached.
    let log = rt.host().borrow().console.clone();
    assert!(
        !log[0].contains("window"),
        "cancelBubble before dispatch must not reach window (got {})",
        log[0]
    );
}

#[test]
fn cancel_bubble_before_dispatch_on_boa() {
    cancel_bubble_before_dispatch::<script_engine_boa::BoaEngine>();
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn event_target_chain_resolves_on_nova() {
    event_target_chain_resolves::<script_engine_nova::NovaEngine>();
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn cancel_bubble_before_dispatch_on_nova() {
    cancel_bubble_before_dispatch::<script_engine_nova::NovaEngine>();
}

/// The Element surface, against any backend: prototype split (`instanceof
/// Element`, `nodeType`), attribute methods (`hasAttribute` / `removeAttribute`
/// / `toggleAttribute`), reflection (`id` / `className`), `classList`, and
/// `querySelector` / `querySelectorAll` / `matches` over a loaded tree.
fn dom_element_surface_works<E: ScriptEngine>() {
    use genet_static_dom::StaticDocument;
    let mut rt = Runtime::<E>::new().expect("runtime");
    rt.load_dom(&StaticDocument::parse(
        "<html><body><div id='a' class='x y'><p class='x'>hi</p><span></span></div></body></html>",
    ));

    rt.eval(
        "var div = document.getElementById('a');\
         console.log(String(div instanceof Element));\
         console.log(String(div.nodeType));\
         console.log(div.id + ',' + div.className);\
         console.log(String(div.hasAttribute('class')) + ',' + String(div.hasAttribute('nope')));\
         div.classList.add('z'); div.classList.remove('y');\
         console.log(div.className + ',' + String(div.classList.contains('x')) + ',' + String(div.classList.length));\
         div.toggleAttribute('hidden');\
         console.log(String(div.hasAttribute('hidden')));\
         console.log(String(document.querySelectorAll('.x').length));\
         console.log(document.querySelector('div > p').textContent);\
         console.log(String(div.querySelectorAll('span').length));\
         console.log(String(document.querySelector('p').matches('.x')));",
    )
    .expect("element script");

    assert_eq!(
        rt.host().borrow().console,
        vec![
            "true",       // div instanceof Element
            "1",          // nodeType ELEMENT_NODE
            "a,x y",      // id, className
            "true,false", // hasAttribute
            "x z,true,2", // className after add('z')/remove('y'); classList has x; length 2
            "true",       // toggleAttribute added 'hidden'
            "2",          // .x matches div + p
            "hi",         // div > p textContent
            "1",          // div's span descendants
            "true",       // p matches .x
        ],
    );
}

#[test]
fn dom_construction_on_boa() {
    dom_construction_works::<script_engine_boa::BoaEngine>();
}

/// Node/Element traversal + mutation, against any backend: child/sibling
/// navigation (incl. element-filtered), `nodeName`/`nodeValue`, `childNodes`,
/// `removeChild` / `insertBefore` / `replaceChild`, and the ChildNode mixin.
fn dom_traversal_works<E: ScriptEngine>() {
    use genet_static_dom::StaticDocument;
    let mut rt = Runtime::<E>::new().expect("runtime");
    rt.load_dom(&StaticDocument::parse(
        "<html><body><div id='p'>text<span id='a'></span><span id='b'></span></div></body></html>",
    ));

    rt.eval(
        "function ids(c){ var o=[]; for (var i=0;i<c.length;i++) o.push(c[i].id); return o.join(','); }\
         var p = document.getElementById('p');\
         console.log(String(p.childNodes.length));\
         console.log(p.firstChild.nodeName + ',' + p.firstChild.nodeValue);\
         console.log(String(p.childElementCount));\
         console.log(p.firstElementChild.id + ',' + p.lastElementChild.id);\
         var a = document.getElementById('a');\
         console.log(a.nextElementSibling.id + ',' + String(a.previousElementSibling));\
         var c = document.createElement('span'); c.id = 'c';\
         p.insertBefore(c, document.getElementById('b'));\
         console.log(ids(p.children));\
         p.removeChild(a);\
         console.log(ids(p.children));\
         var d = document.createElement('span'); d.id = 'd'; c.after(d);\
         console.log(ids(p.children));\
         d.remove();\
         console.log(ids(p.children));\
         console.log(String(p.contains(c)) + ',' + String(p.contains(a)));",
    )
    .expect("traversal script");

    assert_eq!(
        rt.host().borrow().console,
        vec![
            "3",          // childNodes: text + span#a + span#b
            "#text,text", // firstChild nodeName/nodeValue
            "2",          // childElementCount (two spans)
            "a,b",        // first/last element child ids
            "b,null",     // a.nextElementSibling=b, previousElementSibling=null
            "a,c,b",      // after insertBefore(c, b)
            "c,b",        // after removeChild(a)
            "c,d,b",      // after c.after(d)
            "c,b",        // after d.remove()
            "true,false", // contains c (yes), a (removed, no)
        ],
    );
}

#[test]
fn dom_element_surface_on_boa() {
    dom_element_surface_works::<script_engine_boa::BoaEngine>();
}

/// Reflected IDL attributes + namespace getters + createElementNS + tree
/// walker + document.title, against any backend.
fn dom_reflection_ns_works<E: ScriptEngine>() {
    use genet_static_dom::StaticDocument;
    let mut rt = Runtime::<E>::new().expect("runtime");
    rt.load_dom(&StaticDocument::parse(
        "<html><head><title>  Hi   there </title></head><body><a id='x'></a></body></html>",
    ));

    rt.eval(
        "var a = document.getElementById('x');\
         console.log(typeof a.title + ',' + typeof a.hidden + ',' + typeof a.tabIndex);\
         a.title = 'T'; console.log(a.title + ',' + a.getAttribute('title'));\
         a.hidden = true; console.log(String(a.hidden) + ',' + String(a.hasAttribute('hidden')));\
         a.hidden = false; console.log(String(a.hidden));\
         a.tabIndex = 3; console.log(String(a.tabIndex));\
         console.log(a.localName + ',' + a.namespaceURI + ',' + a.tagName);\
         var svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg:rect');\
         console.log(svg.localName + ',' + svg.namespaceURI + ',' + svg.prefix + ',' + svg.tagName);\
         console.log(document.title);\
         console.log(typeof NodeFilter + ',' + NodeFilter.SHOW_ELEMENT);\
         var tw = document.createTreeWalker(document.body, NodeFilter.SHOW_ELEMENT);\
         var seen = []; var n; while ((n = tw.nextNode())) { seen.push(n.localName); }\
         console.log(seen.join(','));",
    )
    .expect("reflection/ns script");

    assert_eq!(
        rt.host().borrow().console,
        vec![
            "string,boolean,number",                        // typeof reflected attrs
            "T,T",                                          // title set reflects to attribute
            "true,true",                                    // hidden boolean reflects
            "false",                                        // hidden cleared
            "3",                                            // tabIndex long roundtrip
            "a,http://www.w3.org/1999/xhtml,A", // localName/namespaceURI/tagName (HTML upper)
            "rect,http://www.w3.org/2000/svg,svg,svg:rect", // createElementNS: localName=rect, tagName=qualified (prefix kept, not upper-cased)
            "Hi there",                                     // document.title whitespace-collapsed
            "object,1",                                     // NodeFilter present
            "a",                                            // tree walker over body finds the <a>
        ],
    );
}

#[test]
fn dom_traversal_on_boa() {
    dom_traversal_works::<script_engine_boa::BoaEngine>();
}

/// `Node.moveBefore` (moveBefore plan S3), against any backend: cross-parent
/// move, same-parent reorder, in-place no-op, the pre-move validity throws
/// (bad reference → NotFoundError; disconnected node and would-be cycle →
/// HierarchyRequestError), the node return value, and a move inside a detached
/// tree (same root, so allowed — moveBefore never adopts, it never crosses).
fn dom_move_before_works<E: ScriptEngine>() {
    use genet_static_dom::StaticDocument;
    let mut rt = Runtime::<E>::new().expect("runtime");
    rt.load_dom(&StaticDocument::parse(
        "<html><body><div id='a'><p id='p'></p></div>\
         <div id='b'><span id='bk'></span></div></body></html>",
    ));

    rt.eval(
        "function ids(c){ var o=[]; for (var i=0;i<c.length;i++) o.push(c[i].id); return o.join(','); }\
         var a = document.getElementById('a');\
         var b = document.getElementById('b');\
         var p = document.getElementById('p');\
         var bk = document.getElementById('bk');\
         b.moveBefore(p, null);\
         console.log(p.parentNode.id + ':' + ids(b.children) + ':' + ids(a.children));\
         b.moveBefore(p, bk);\
         console.log(ids(b.children));\
         b.moveBefore(p, bk);\
         console.log(ids(b.children));\
         try { b.moveBefore(p, a); } catch (e) { console.log(e.name); }\
         var d = document.createElement('div');\
         try { b.moveBefore(d, null); } catch (e) { console.log(e.name); }\
         try { p.moveBefore(b, null); } catch (e) { console.log(e.name); }\
         console.log(String(b.moveBefore(p, null) === p) + ':' + ids(b.children));\
         var host = document.createElement('div');\
         var x = document.createElement('span'); x.id = 'x';\
         var y = document.createElement('span'); y.id = 'y';\
         host.appendChild(x); host.appendChild(y);\
         host.moveBefore(y, x);\
         console.log(ids(host.children));",
    )
    .expect("moveBefore script");

    assert_eq!(
        rt.host().borrow().console,
        vec![
            "b:bk,p:",               // cross-parent append: p under b, a emptied
            "p,bk",                  // reorder before bk
            "p,bk",                  // in-place move is a no-op
            "NotFoundError",         // reference not a child of the target
            "HierarchyRequestError", // a disconnected node cannot move in
            "HierarchyRequestError", // moving an ancestor under its descendant
            "true:bk,p",             // returns the node; p moved to the end
            "y,x",                   // a move inside one detached tree is legal
        ],
    );
}

#[test]
fn dom_move_before_on_boa() {
    dom_move_before_works::<script_engine_boa::BoaEngine>();
}

#[test]
fn dom_reflection_ns_on_boa() {
    dom_reflection_ns_works::<script_engine_boa::BoaEngine>();
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn dom_reflection_ns_on_nova() {
    dom_reflection_ns_works::<script_engine_nova::NovaEngine>();
}

/// HTML interface table: per-tag constructors/prototypes and reflected IDL attrs
/// come from the declarative table, not hand-written element cases.
fn html_interface_table_works<E: ScriptEngine>() {
    use genet_static_dom::StaticDocument;
    let mut rt = Runtime::<E>::new().expect("runtime");
    rt.set_base_url("http://example.test/base/index.html")
        .expect("base url");
    rt.load_dom(&StaticDocument::parse(
        "<html><body><a id='a' href='p'></a><button id='b'></button><canvas id='c'></canvas><video id='v'></video><audio id='au'></audio></body></html>",
    ));

    rt.eval(
        "var a = document.getElementById('a');\
         var b = document.getElementById('b');\
         var c = document.getElementById('c');\
         var v = document.getElementById('v');\
         var au = document.getElementById('au');\
         var div = document.createElement('div');\
         console.log(typeof HTMLAnchorElement + ',' + typeof HTMLButtonElement + ',' + typeof HTMLCanvasElement + ',' + typeof HTMLMediaElement);\
         console.log(HTMLCanvasElement.name + ',' + HTMLButtonElement.name + ',' + HTMLMediaElement.name);\
         console.log(String(a instanceof HTMLAnchorElement) + ',' + String(b instanceof HTMLButtonElement) + ',' + String(c instanceof HTMLCanvasElement) + ',' + String(c instanceof HTMLElement));\
         console.log(String(a instanceof HTMLButtonElement) + ',' + String(b instanceof HTMLAnchorElement));\
         console.log(a.href + ',' + String('href' in div));\
         a.text = 'go'; console.log(a.text + ',' + a.textContent + ',' + String(a.getAttribute('text')));\
         b.disabled = true; console.log(String(b.disabled) + ',' + String(b.hasAttribute('disabled')) + ',' + String('disabled' in div));\
         console.log(String(v instanceof HTMLVideoElement) + ',' + String(v instanceof HTMLMediaElement) + ',' + String(au instanceof HTMLAudioElement) + ',' + String(au instanceof HTMLMediaElement));\
         v.src = 'clip.mp4'; au.controls = true; console.log(v.getAttribute('src') + ',' + String(au.hasAttribute('controls')));\
         console.log(String(c.width) + ',' + String(c.height));\
         c.width = 640; console.log(c.getAttribute('width') + ',' + String(c.width));\
         console.log(String(c.getContext === HTMLCanvasElement.prototype.getContext));\
         console.log(String(document.createElement('BUTTON') instanceof HTMLButtonElement));\
         var svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg:rect');\
         console.log(String(svg instanceof Element) + ',' + String(svg instanceof HTMLElement));",
    )
    .expect("html interface table script");

    assert_eq!(
        rt.host().borrow().console,
        vec![
            "function,function,function,function",
            "HTMLCanvasElement,HTMLButtonElement,HTMLMediaElement",
            "true,true,true,true",
            "false,false",
            "http://example.test/base/p,false",
            "go,go,null",
            "true,true,false",
            "true,true,true,true",
            "clip.mp4,true",
            "300,150",
            "640,640",
            "true",
            "true",
            "true,false",
        ],
    );
}

#[test]
fn html_interface_table_on_boa() {
    html_interface_table_works::<script_engine_boa::BoaEngine>();
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn html_interface_table_on_nova() {
    html_interface_table_works::<script_engine_nova::NovaEngine>();
}

/// Focused CustomElementRegistry follow-on slice: tighten custom-element name
/// validation, keep pending whenDefined promises stable, type-check getName,
/// and match the constructor/prototype property access order expected by the
/// WPT registry surface.
fn custom_elements_registry_contract_works<E: ScriptEngine>() {
    use genet_static_dom::StaticDocument;
    let mut rt = Runtime::<E>::new().expect("runtime");
    rt.load_dom(&StaticDocument::parse("<html><body></body></html>"));

    rt.eval(
        "globalThis.registryLog = [];\
         function thrown(fn){ try { fn(); return 'no-throw'; } catch(e){ return e.name; } }\
         console.log(thrown(function(){ customElements.getName(undefined); }));\
         console.log(thrown(function(){ customElements.define('a-Bc', function BadUpper(){}); }));\
         console.log(thrown(function(){ customElements.define('annotation-xml', function Reserved(){}); }));\
         console.log(String(customElements.whenDefined('pending-el') === customElements.whenDefined('pending-el')));\
         customElements.whenDefined('badname').then(function(){ registryLog.push('invalid:resolved'); }, function(e){ registryLog.push('invalid:' + e.name); });\
         var ctorCalls = [];\
         function Ordered(){}\
         Ordered.prototype.attributeChangedCallback = function(){};\
         Ordered.prototype = new Proxy(Ordered.prototype, { get: function(target, name){ registryLog.push('proto:' + String(name)); return target[name]; } });\
         var OrderedProxy = new Proxy(Ordered, { get: function(target, name){ ctorCalls.push(String(name)); return target[name]; } });\
         customElements.define('ordered-element', OrderedProxy);\
         console.log('ctor:' + ctorCalls.join('|'));\
         var noObservedCalls = [];\
         function NoObserved(){}\
         var NoObservedProxy = new Proxy(NoObserved, { get: function(target, name){ noObservedCalls.push(String(name)); if (name === 'observedAttributes') return 1; return target[name]; } });\
         customElements.define('no-observed-element', NoObservedProxy);\
         console.log('no-observed:' + noObservedCalls.join('|'));\
         customElements.define('dupe-element', function FirstDupe(){});\
         var dupeCalls = [];\
         var DupeProxy = new Proxy(function DupeProxy(){}, { get: function(target, name){ dupeCalls.push(String(name)); return target[name]; } });\
         console.log(thrown(function(){ customElements.define('dupe-element', DupeProxy); }) + ',' + String(dupeCalls.length));\
         var outerCalls = [];\
         var Outer = new Proxy(function Outer(){}, { get: function(target, name){ outerCalls.push(String(name)); customElements.define('inner-running-element', function Inner(){}); return target[name]; } });\
         console.log(thrown(function(){ customElements.define('outer-running-element', Outer); }));\
         console.log('outer:' + outerCalls.join('|'));\
         class BuiltIn extends HTMLButtonElement {}\
         customElements.define('named-builtin', BuiltIn, { extends: 'button' });\
         console.log(customElements.getName(BuiltIn));\
         class ResolvedEl extends HTMLElement {}\
         customElements.define('resolved-el', ResolvedEl);\
         customElements.whenDefined('resolved-el').then(function(value){ registryLog.push('resolved:' + String(value === ResolvedEl)); });\
         class LaterEl extends HTMLElement {}\
         customElements.whenDefined('later-el').then(function(value){ registryLog.push('later:' + String(value === LaterEl)); });\
         customElements.define('later-el', LaterEl);",
    )
    .expect("custom elements registry setup script");

    rt.run_microtasks();
    rt.eval("console.log('async:' + registryLog.join('|'));")
        .expect("custom elements registry async script");

    assert_eq!(
        rt.host().borrow().console,
        vec![
            "TypeError",
            "SyntaxError",
            "SyntaxError",
            "true",
            "ctor:prototype|observedAttributes|disabledFeatures|formAssociated",
            "no-observed:prototype|disabledFeatures|formAssociated",
            "NotSupportedError,0",
            "NotSupportedError",
            "outer:prototype",
            "named-builtin",
            "async:proto:connectedCallback|proto:disconnectedCallback|proto:adoptedCallback|proto:attributeChangedCallback|invalid:SyntaxError|resolved:true|later:true",
        ],
    );
}

#[test]
fn custom_elements_registry_contract_on_boa() {
    custom_elements_registry_contract_works::<script_engine_boa::BoaEngine>();
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn custom_elements_registry_contract_on_nova() {
    custom_elements_registry_contract_works::<script_engine_nova::NovaEngine>();
}

/// HTML constructor follow-on slice: direct `new` / `Reflect.construct`
/// construction now consults the custom-element definition table instead of
/// only the upgrade-time construction stack, while still preserving the WPT
/// sanity-check ordering around `NewTarget.prototype`.
fn custom_elements_html_constructor_works<E: ScriptEngine>() {
    use genet_static_dom::StaticDocument;
    let mut rt = Runtime::<E>::new().expect("runtime");
    rt.load_dom(&StaticDocument::parse("<html><body></body></html>"));

    rt.eval(
        "function thrown(fn){ try { fn(); return 'no-throw'; } catch(e){ return e.name; } }\
         console.log(thrown(function(){ new HTMLElement(); }));\
         class NotDefined extends HTMLElement {}\
         console.log(thrown(function(){ new NotDefined(); }));\
         class WrongAutonomous extends HTMLParagraphElement {}\
         customElements.define('wrong-autonomous', WrongAutonomous);\
         console.log(thrown(function(){ new WrongAutonomous(); }));\
         class WrongBuiltIn extends HTMLButtonElement {}\
         customElements.define('wrong-built-in', WrongBuiltIn, { extends: 'p' });\
         console.log(thrown(function(){ new WrongBuiltIn(); }));\
         class FancyEl extends HTMLElement {}\
         customElements.define('fancy-el', FancyEl);\
         var fancy = new FancyEl();\
         console.log(String(fancy instanceof FancyEl) + ',' + String(fancy instanceof Element) + ',' + fancy.localName + ',' + fancy.nodeName);\
         class SubFancy extends FancyEl {}\
         customElements.define('sub-fancy', SubFancy);\
         var sub = new SubFancy();\
         console.log(String(sub instanceof FancyEl) + ',' + String(sub instanceof SubFancy) + ',' + sub.localName);\
         class FancyButton extends HTMLButtonElement { constructor(){ super(); this.ready = 'yes'; } }\
         customElements.define('fancy-button', FancyButton, { extends: 'button' });\
         var button = new FancyButton();\
         console.log(String(button instanceof FancyButton) + ',' + String(button instanceof HTMLButtonElement) + ',' + button.localName + ',' + button.getAttribute('is') + ',' + button.ready);\
         class PlainCtor {}\
         customElements.define('plain-ctor-el', PlainCtor);\
         var plain = Reflect.construct(HTMLElement, [], PlainCtor);\
         console.log(String(plain instanceof PlainCtor) + ',' + String(plain.localName) + ',' + String(plain.nodeName));\
         class FailureCtor extends HTMLElement {}\
         customElements.define('failure-counting-element', FailureCtor, { extends: 'button' });\
         console.log(thrown(function(){ Reflect.construct(HTMLElement, [], FailureCtor); }));\
         class FailureParagraph extends HTMLParagraphElement {}\
         customElements.define('failure-counting-paragraph', FailureParagraph);\
         console.log(thrown(function(){ Reflect.construct(HTMLParagraphElement, [], FailureParagraph); }));",
    )
    .expect("custom elements html constructor script");

    assert_eq!(
        rt.host().borrow().console,
        vec![
            "TypeError",
            "TypeError",
            "TypeError",
            "TypeError",
            "true,true,fancy-el,FANCY-EL",
            "true,true,sub-fancy",
            "true,true,button,fancy-button,yes",
            "true,undefined,undefined",
            "TypeError",
            "TypeError",
        ],
    );
}

#[test]
fn custom_elements_html_constructor_on_boa() {
    custom_elements_html_constructor_works::<script_engine_boa::BoaEngine>();
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn custom_elements_html_constructor_on_nova() {
    custom_elements_html_constructor_works::<script_engine_nova::NovaEngine>();
}

/// First custom-elements I3 slice: use the HTML interface table for customized
/// built-ins, upgrade existing matching nodes on define, support `{ is }`
/// creation, explicit upgrade of detached subtrees, and Promise-microtask-timed
/// connected / disconnected / attribute reactions.
fn custom_elements_customized_builtins_work<E: ScriptEngine>() {
    use genet_static_dom::StaticDocument;
    let mut rt = Runtime::<E>::new().expect("runtime");
    rt.load_dom(&StaticDocument::parse(
        "<html><body><button id='old' is='x-fancy' data-state='seed'></button><button id='plain'></button><x-card id='card'></x-card></body></html>",
    ));

    rt.eval(
        "globalThis.ceLog = [];\
         function logReaction(s){ ceLog.push(s); }\
         function FancyButton(){ this.setAttribute('upgraded', 'yes'); }\
         FancyButton.observedAttributes = ['data-state', 'disabled'];\
         FancyButton.prototype = Object.create(HTMLButtonElement.prototype);\
         FancyButton.prototype.constructor = FancyButton;\
         FancyButton.prototype.connectedCallback = function(){ logReaction('connected:' + this.id); this.setAttribute('connected', 'yes'); };\
         FancyButton.prototype.disconnectedCallback = function(){ logReaction('disconnected:' + this.id); };\
         FancyButton.prototype.attributeChangedCallback = function(name, oldValue, newValue){ logReaction('attr:' + this.id + ':' + name + ':' + String(oldValue) + '>' + String(newValue)); };\
         customElements.define('x-fancy', FancyButton, { extends: 'button' });\
         var old = document.getElementById('old');\
         console.log(String(old instanceof FancyButton) + ',' + String(old instanceof HTMLButtonElement) + ',' + old.getAttribute('upgraded') + ',' + old.getAttribute('connected'));\
         old.setAttribute('data-state', 'hot');\
         old.disabled = true;\
         var made = document.createElement('button', { is: 'x-fancy' });\
         console.log(made.getAttribute('is') + ',' + String(made instanceof FancyButton) + ',' + made.getAttribute('upgraded') + ',' + String(made.isConnected));\
         document.body.appendChild(made);\
         document.body.removeChild(made);\
         console.log(String(made.isConnected) + ',' + String(made.getAttribute('connected')));\
         console.log(String(document.getElementById('plain') instanceof FancyButton));\
         function XCard(){ this.flag = 'card'; }\
         XCard.prototype = Object.create(HTMLElement.prototype);\
         XCard.prototype.constructor = XCard;\
         XCard.prototype.connectedCallback = function(){ logReaction('connected-card:' + this.id); };\
         customElements.define('x-card', XCard);\
         var card = document.getElementById('card');\
         console.log(String(card instanceof XCard) + ',' + String(card instanceof HTMLElement) + ',' + card.flag);\
         var detached = document.createElement('button', { is: 'x-late' });\
         function LateButton(){ this.setAttribute('late', 'yes'); }\
         LateButton.prototype = Object.create(HTMLButtonElement.prototype);\
         LateButton.prototype.constructor = LateButton;\
         customElements.define('x-late', LateButton, { extends: 'button' });\
         console.log(String(detached instanceof LateButton));\
         customElements.upgrade(detached);\
         console.log(String(detached instanceof LateButton) + ',' + detached.getAttribute('late'));\
         class ClassButton extends HTMLButtonElement { constructor(){ super(); this.classReady = 'ok'; } }\
         customElements.define('x-classy', ClassButton, { extends: 'button' });\
         var classy = document.createElement('button', { is: 'x-classy' });\
         console.log(String(classy instanceof ClassButton) + ',' + String(classy instanceof HTMLButtonElement) + ',' + classy.classReady);\
         console.log(String(customElements.get('x-fancy') === FancyButton) + ',' + customElements.getName(FancyButton));\
         console.log('events-sync:' + ceLog.length);",
    )
    .expect("custom elements customized built-ins script");

    rt.run_microtasks();
    rt.eval(
        "console.log('events:' + ceLog.join('|'));\
         console.log('post:' + document.getElementById('old').getAttribute('connected') + ',' + String(ceLog.length));",
    )
    .expect("custom elements reaction log script");

    assert_eq!(
        rt.host().borrow().console,
        vec![
            "true,true,yes,null",
            "x-fancy,true,yes,false",
            "false,null",
            "false",
            "true,true,card",
            "false",
            "true,yes",
            "true,true,ok",
            "true,x-fancy",
            "events-sync:0",
            "events:attr:old:data-state:null>seed|connected:old|attr:old:data-state:seed>hot|attr:old:disabled:null>|connected:|disconnected:|connected-card:card",
            "post:yes,7",
        ],
    );
}

#[test]
fn custom_elements_customized_builtins_on_boa() {
    custom_elements_customized_builtins_work::<script_engine_boa::BoaEngine>();
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn custom_elements_customized_builtins_on_nova() {
    custom_elements_customized_builtins_work::<script_engine_nova::NovaEngine>();
}

/// Minimal adoption slice over the HTML interface table bootstrap: track
/// ownerDocument across detached documents, enqueue adoptedCallback on
/// cross-document moves/adoptNode, and keep the callback ordering aligned with
/// the existing connected/disconnected microtask queue.
fn custom_elements_adoption_works<E: ScriptEngine>() {
    use genet_static_dom::StaticDocument;
    let mut rt = Runtime::<E>::new().expect("runtime");
    rt.load_dom(&StaticDocument::parse("<html><body></body></html>"));

    rt.eval(
        "document.title = 'main';\
         globalThis.adoptLog = [];\
         function logAdopt(s){ adoptLog.push(s); }\
         function XAdopt(){}\
         XAdopt.prototype = Object.create(HTMLElement.prototype);\
         XAdopt.prototype.constructor = XAdopt;\
         XAdopt.prototype.connectedCallback = function(){ logAdopt('connected:' + this.ownerDocument.title); };\
         XAdopt.prototype.disconnectedCallback = function(){ logAdopt('disconnected'); };\
         XAdopt.prototype.adoptedCallback = function(oldDocument, newDocument){ logAdopt('adopted:' + oldDocument.title + '>' + newDocument.title); };\
         customElements.define('x-adopt', XAdopt);\
         globalThis.otherDoc = document.implementation.createHTMLDocument('other');\
         globalThis.node = document.createElement('x-adopt');\
         console.log(String(node.ownerDocument === document));\
         document.body.appendChild(node);",
    )
    .expect("custom elements adoption setup script");

    rt.run_microtasks();
    rt.eval(
        "console.log('step1:' + adoptLog.join('|'));\
         adoptLog = [];\
         otherDoc.body.appendChild(node);",
    )
    .expect("custom elements adoption move script");

    rt.run_microtasks();
    rt.eval(
        "console.log(String(node.ownerDocument === otherDoc));\
         console.log('step2:' + adoptLog.join('|'));\
         adoptLog = [];\
         globalThis.detached = otherDoc.createElement('x-adopt');\
         console.log(String(detached.ownerDocument === otherDoc));\
         document.adoptNode(detached);",
    )
    .expect("custom elements detached adoptNode script");

    rt.run_microtasks();
    rt.eval(
        "console.log(String(detached.ownerDocument === document));\
         console.log('step3:' + adoptLog.join('|'));\
         adoptLog = [];\
         globalThis.connected = otherDoc.createElement('x-adopt');\
         otherDoc.body.appendChild(connected);",
    )
    .expect("custom elements connected setup script");

    rt.run_microtasks();
    rt.eval(
        "adoptLog = [];\
         document.adoptNode(connected);",
    )
    .expect("custom elements connected adoptNode script");

    rt.run_microtasks();
    rt.eval(
        "console.log(String(connected.ownerDocument === document) + ',' + String(connected.isConnected));\
         console.log('step4:' + adoptLog.join('|'));\
         adoptLog = [];\
         globalThis.parent = document.createElement('div');\
         globalThis.descendant = document.createElement('x-adopt');\
         parent.appendChild(descendant);\
         otherDoc.body.appendChild(parent);",
    )
    .expect("custom elements ancestor move setup script");

    rt.run_microtasks();
    rt.eval(
        "console.log(String(descendant.ownerDocument === otherDoc));\
         console.log('step5:' + adoptLog.join('|'));",
    )
    .expect("custom elements ancestor move script");

    assert_eq!(
        rt.host().borrow().console,
        vec![
            "true",
            "step1:connected:main",
            "true",
            "step2:disconnected|adopted:main>other|connected:other",
            "true",
            "true",
            "step3:adopted:other>main",
            "true,false",
            "step4:disconnected|adopted:other>main",
            "true",
            "step5:adopted:main>other|connected:other",
        ],
    );
}

#[test]
fn custom_elements_adoption_on_boa() {
    custom_elements_adoption_works::<script_engine_boa::BoaEngine>();
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn custom_elements_adoption_on_nova() {
    custom_elements_adoption_works::<script_engine_nova::NovaEngine>();
}

/// Probe: does the backend's `Proxy` support the traps a live HTMLCollection
/// needs (get for integer index, has, ownKeys)? Determines whether the exotic
/// collection can be a JS Proxy in the bootstrap vs needing an engine primitive.
fn proxy_capability<E: ScriptEngine>() {
    let mut rt = Runtime::<E>::new().expect("runtime");
    rt.eval(
        "var p = new Proxy({}, {\
           get: function(t, k) { if (k === '0') return 'zero'; if (k === 'length') return 1; return undefined; },\
           has: function(t, k) { return k === '0'; },\
           ownKeys: function(t) { return ['0', 'length']; },\
           getOwnPropertyDescriptor: function(t, k) { return { value: (k==='0'?'zero':1), enumerable: true, configurable: true }; }\
         });\
         console.log(String(p[0]));\
         console.log(String(p.length));\
         console.log(String('0' in p));\
         console.log(Object.keys(p).join(','));",
    )
    .expect("proxy eval");
    assert_eq!(
        rt.host().borrow().console,
        vec!["zero", "1", "true", "0,length"]
    );
}

/// Live HTMLCollection / NodeList exotic semantics, against any backend:
/// liveness, item/namedItem, indexed + named access, Symbol.iterator, the
/// HTMLCollection-lacks-forEach / NodeList-has-forEach distinction, and
/// getOwnPropertyNames order (indices then deduped non-empty id/name).
fn dom_collections_works<E: ScriptEngine>() {
    use genet_static_dom::StaticDocument;
    let mut rt = Runtime::<E>::new().expect("runtime");
    rt.load_dom(&StaticDocument::parse(
        "<html><body><span id='x'></span><span name='y'></span></body></html>",
    ));

    rt.eval(
        "var c = document.getElementsByTagName('span');\
         console.log(String(c.length) + ',' + c[0].id + ',' + c.item(1).getAttribute('name'));\
         console.log(c.namedItem('x').id + ',' + String(c.namedItem('nope')));\
         console.log(c['y'].getAttribute('name'));\
         console.log(String(Symbol.iterator in c) + ',' + String('forEach' in c) + ',' + String('values' in c));\
         console.log(Object.getOwnPropertyNames(c).join(','));\
         var seen = []; for (var i = 0; i < c.length; i++) seen.push(c[i].nodeName); \
         var it = ''; var a = c; for (var k = 0; k < a.length; k++) {} \
         document.body.appendChild(document.createElement('span'));\
         console.log(String(c.length));\
         var nl = document.body.childNodes;\
         console.log(String(nl.length) + ',' + String(typeof nl.forEach) + ',' + String('namedItem' in nl));\
         var acc = []; nl.forEach(function(n){ acc.push(n.nodeName); });\
         console.log(acc.join(','));",
    )
    .expect("collections script");

    assert_eq!(
        rt.host().borrow().console,
        vec![
            "2,x,y",            // length, [0].id, item(1) name
            "x,null",           // namedItem hit / miss
            "y",                // named access c['y']
            "true,false,false", // Symbol.iterator yes; forEach/values no (HTMLCollection)
            "0,1,x,y",          // getOwnPropertyNames: indices 0,1 then names x,y
            "3",                // live: after appending a third span
            "3,function,false", // childNodes NodeList: 3 kids, has forEach, no namedItem
            "SPAN,SPAN,SPAN",   // forEach over the NodeList
        ],
    );
}

#[test]
fn proxy_capability_on_boa() {
    proxy_capability::<script_engine_boa::BoaEngine>();
}

/// DOMTokenList (classList/relList) + dataset exotics, against any backend:
/// the iterable surface + brand + value + indexed access + replace, and
/// dataset camelCase<->kebab get/set/has/keys.
fn dom_tokenlist_dataset_works<E: ScriptEngine>() {
    use genet_static_dom::StaticDocument;
    let mut rt = Runtime::<E>::new().expect("runtime");
    rt.load_dom(&StaticDocument::parse(
        "<html><body><a id='a' class='x y'></a></body></html>",
    ));

    rt.eval(
        "var a = document.getElementById('a');\
         var cl = a.classList;\
         console.log(Object.prototype.toString.call(cl));\
         console.log(String(typeof cl.values) + ',' + String(typeof cl.forEach) + ',' + String(Symbol.iterator in cl));\
         console.log(cl.value + ',' + String(cl.length) + ',' + cl[0] + ',' + cl[1]);\
         console.log(String(cl.replace('x', 'z')) + ',' + cl.value);\
         var seen = []; cl.forEach(function(t){ seen.push(t); }); console.log(seen.join(','));\
         a.rel = 'next prev'; console.log(String(a.relList.length) + ',' + a.relList.contains('prev'));\
         a.dataset.fooBar = 'v'; console.log(a.getAttribute('data-foo-bar'));\
         a.setAttribute('data-baz', 'w'); console.log(a.dataset.baz);\
         console.log(String('fooBar' in a.dataset) + ',' + String('nope' in a.dataset));\
         console.log(Object.keys(a.dataset).sort().join(','));\
         delete a.dataset.baz; console.log(String(a.hasAttribute('data-baz')));",
    )
    .expect("tokenlist/dataset script");

    assert_eq!(
        rt.host().borrow().console,
        vec![
            "[object DOMTokenList]",  // brand
            "function,function,true", // values, forEach, Symbol.iterator
            "x y,2,x,y",              // value, length, [0], [1]
            "true,z y",               // replace('x','z') -> 'z y'
            "z,y",                    // forEach over tokens
            "2,true",                 // relList from rel='next prev'
            "v",                      // dataset.fooBar -> data-foo-bar
            "w",                      // data-baz -> dataset.baz
            "true,false",             // 'fooBar' in dataset, 'nope' not
            "baz,fooBar",             // Object.keys(dataset) (sorted)
            "false",                  // delete dataset.baz removed the attr
        ],
    );
}

/// URL reflected IDL attributes (`href`, `src`, …): the getter resolves the
/// content attribute against the document base URL, the setter stores the raw
/// string, and an absent attribute reflects as the empty string.
fn dom_url_reflection_works<E: ScriptEngine>() {
    use genet_static_dom::StaticDocument;
    let mut rt = Runtime::<E>::new().expect("runtime");
    rt.set_base_url("http://example.com/dir/page.html")
        .expect("base url");
    rt.load_dom(&StaticDocument::parse(
        "<html><body><a id='a' href='sub/x.html'></a><img id='i'></body></html>",
    ));
    rt.eval(
        "var a = document.getElementById('a');\
         console.log(a.href);\
         a.href = 'other.html'; console.log(a.getAttribute('href'));\
         console.log(a.href);\
         console.log(document.getElementById('i').src);",
    )
    .expect("url-reflection script");
    assert_eq!(
        rt.host().borrow().console,
        vec![
            "http://example.com/dir/sub/x.html", // relative href resolved on get
            "other.html",                        // setter stored the raw value
            "http://example.com/dir/other.html", // re-resolved after set
            "",                                  // absent src reflects as ""
        ],
    );
}

#[test]
fn dom_collections_on_boa() {
    dom_collections_works::<script_engine_boa::BoaEngine>();
}
#[test]
fn dom_url_reflection_on_boa() {
    dom_url_reflection_works::<script_engine_boa::BoaEngine>();
}

/// DOMImplementation + multi-document, against any backend: hasFeature,
/// createHTMLDocument (with title + body), createDocument with a root element,
/// and queries scoping to the created document, not the primary one.
fn dom_implementation_works<E: ScriptEngine>() {
    use genet_static_dom::StaticDocument;
    let mut rt = Runtime::<E>::new().expect("runtime");
    rt.load_dom(&StaticDocument::parse(
        "<html><body><p id='main'></p></body></html>",
    ));

    rt.eval(
        "console.log(String(document.implementation.hasFeature('x', 'y')));\
         var d = document.implementation.createHTMLDocument('Hello');\
         console.log(d.title + ',' + d.documentElement.tagName + ',' + (d.body ? d.body.tagName : 'no-body'));\
         var p = d.createElement('p'); p.id = 'sub'; d.body.appendChild(p);\
         console.log(d.getElementById('sub') ? d.getElementById('sub').id : 'not-found');\
         console.log(String(document.getElementById('sub')));\
         console.log(String(document.getElementById('main') ? 'main-here' : 'no-main'));\
         var xml = document.implementation.createDocument('urn:ns', 'root', null);\
         console.log(xml.documentElement.tagName + ',' + xml.documentElement.namespaceURI);",
    )
    .expect("implementation script");

    assert_eq!(
        rt.host().borrow().console,
        vec![
            "true",            // hasFeature always true
            "Hello,HTML,BODY", // createHTMLDocument: title, <html>, <body>
            "sub",             // getElementById scoped to the created doc
            "null",            // primary document does NOT see the created doc's #sub
            "main-here",       // primary document still finds its own #main
            "root,urn:ns",     // createDocument: root element + namespace
        ],
    );
}

/// An iframe owns a stable initial child document/window pair. Fragment
/// replacement and queries stay scoped to that document, while hosts that do
/// not specialize nested style contexts inherit the ordinary computed-style
/// handler behavior.
fn iframe_initial_document_works<E: ScriptEngine>() {
    use genet_static_dom::StaticDocument;

    struct Stub;
    impl crate::ComputedStyleHandler for Stub {
        fn computed_value(&self, _node: u64, property: &str) -> Option<String> {
            (property == "color").then(|| "rgb(1, 2, 3)".to_string())
        }
    }

    let mut rt = Runtime::<E>::new().expect("runtime");
    rt.load_dom(&StaticDocument::parse(
        "<html><body><iframe id='frame'></iframe></body></html>",
    ));
    rt.set_computed_style_handler(Box::new(Stub));
    rt.eval(
        "var child = frame.contentDocument; var win = frame.contentWindow;\
         child.body.innerHTML = '<style>div { color: red; }</style><div id=\"inside\">ok</div>';\
         console.log(String(child === frame.contentDocument) + '|' +\
           String(win === frame.contentWindow) + '|' + String(win.document === child));\
         console.log(child.body.innerHTML);\
         console.log(child.querySelector('#inside').textContent + '|' +\
           String(document.querySelector('#inside')) + '|' +\
           win.getComputedStyle(child.querySelector('#inside')).color);",
    )
    .expect("iframe initial-document script");

    assert_eq!(
        rt.host().borrow().console,
        vec![
            "true|true|true",
            "<style>div { color: red; }</style><div id=\"inside\">ok</div>",
            "ok|null|rgb(1, 2, 3)",
        ],
    );
}

#[test]
fn iframe_initial_document_on_boa() {
    iframe_initial_document_works::<script_engine_boa::BoaEngine>();
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn iframe_initial_document_on_nova() {
    iframe_initial_document_works::<script_engine_nova::NovaEngine>();
}

/// `element.style` is a CSSStyleDeclaration over the inline `style` attribute:
/// getPropertyValue / camelCase get + set / setProperty / removeProperty /
/// length / item / cssText / `in`, all writing back to the attribute.
fn element_style_inline_cssom<E: ScriptEngine>() {
    use genet_static_dom::StaticDocument;
    let mut rt = Runtime::<E>::new().expect("runtime");
    rt.load_dom(&StaticDocument::parse(
        "<html><body><div id='d' style='color: red; font-size: 12px'></div></body></html>",
    ));
    rt.eval(
        "var s = document.getElementById('d').style;\
         console.log(s.color + ',' + s.fontSize + ',' + s.getPropertyValue('font-size'));\
         console.log(s.length + ',' + s.item(0) + ',' + s.item(1));\
         s.color = 'blue'; s.setProperty('margin-top', '4px'); s.fontWeight = 'bold';\
         console.log(document.getElementById('d').getAttribute('style'));\
         console.log(s.removeProperty('font-size') + ',' + ('color' in s) + ',' + ('display' in s));\
         s.cssText = 'padding: 1px; color: green';\
         console.log(s.cssText + ',' + s.color);\
         document.getElementById('d').style = 'width: 7px';\
         console.log(document.getElementById('d').style.cssText);",
    )
    .expect("style script");
    assert_eq!(
        rt.host().borrow().console,
        vec![
            "red,12px,12px",     // .color, .fontSize (camelCase), getPropertyValue
            "2,color,font-size", // length, item(0), item(1)
            "color: blue; font-size: 12px; margin-top: 4px; font-weight: bold;",
            "12px,true,false", // removeProperty returns old; 'color' in / 'display' in
            "padding: 1px; color: green;,green", // cssText set + read; .color
            "width: 7px;",     // [PutForwards=cssText] assignment
        ],
    );
}

#[test]
fn element_style_inline_cssom_on_boa() {
    element_style_inline_cssom::<script_engine_boa::BoaEngine>();
}
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn element_style_inline_cssom_on_nova() {
    element_style_inline_cssom::<script_engine_nova::NovaEngine>();
}

/// A selected CSS engine can canonicalize supported inline values, reject
/// values it can prove invalid, and pass unfamiliar syntax through unchanged.
fn element_style_routes_through_inline_handler<E: ScriptEngine>() {
    use genet_static_dom::StaticDocument;

    struct Stub;
    impl crate::InlineStyleHandler for Stub {
        fn canonicalize(&self, property: &str, value: &str) -> crate::InlineStyleValueResult {
            if value == "invalid" {
                crate::InlineStyleValueResult::Invalid
            } else if property == "color" && value.eq_ignore_ascii_case("red") {
                crate::InlineStyleValueResult::Canonical("#ff0000".to_string())
            } else {
                crate::InlineStyleValueResult::PassThrough
            }
        }
    }

    let mut rt = Runtime::<E>::new().expect("runtime");
    rt.load_dom(&StaticDocument::parse(
        "<html><body><div id='d' style='color: red; width: Raw'></div></body></html>",
    ));
    rt.set_inline_style_handler(Box::new(Stub));
    rt.eval(
        "var s = document.getElementById('d').style;\
         console.log(s.color + '|' + s.width);\
         s.color = 'invalid'; s.marginTop = '4PX';\
         console.log(s.color + '|' + s.marginTop);\
         s.cssText = 'color: red; height: invalid; width: Raw';\
         console.log(s.cssText);\
         console.log(String(CSS.supports('color', 'red')) + '|' +\
           String(CSS.supports('color: invalid')) + '|' +\
           String(CSS.supports('width', 'Raw')) + '|' +\
           String(CSS.supports('--token', 'anything')));",
    )
    .expect("inline style handler script");
    assert_eq!(
        rt.host().borrow().console,
        vec![
            "#ff0000|Raw",
            "#ff0000|4PX",
            "color: #ff0000; width: Raw;",
            "true|false|false|true",
        ]
    );
}

#[test]
fn element_style_routes_through_inline_handler_on_boa() {
    element_style_routes_through_inline_handler::<script_engine_boa::BoaEngine>();
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn element_style_routes_through_inline_handler_on_nova() {
    element_style_routes_through_inline_handler::<script_engine_nova::NovaEngine>();
}

/// `getComputedStyle(el)` reads through the host `ComputedStyleHandler` seam:
/// supported longhands resolve (camelCase + getPropertyValue), unsupported
/// ones yield "", and the declaration is read-only.
fn get_computed_style_reads_handler<E: ScriptEngine>() {
    use genet_static_dom::StaticDocument;
    struct Stub;
    impl crate::ComputedStyleHandler for Stub {
        fn computed_value(&self, _node: u64, property: &str) -> Option<String> {
            match property {
                "color" => Some("rgb(0, 0, 0)".to_string()),
                "display" => Some("block".to_string()),
                "font-size" => Some("16px".to_string()),
                _ => None,
            }
        }
    }
    let mut rt = Runtime::<E>::new().expect("runtime");
    rt.load_dom(&StaticDocument::parse(
        "<html><body><div id='d'></div></body></html>",
    ));
    rt.set_computed_style_handler(Box::new(Stub));
    rt.eval(
        "var cs = getComputedStyle(document.getElementById('d'));\
         console.log(cs.color + ',' + cs.fontSize + ',' + cs.getPropertyValue('display'));\
         console.log(cs.getPropertyValue('margin-top') + '|' + cs.marginTop + '|' + cs.bogus);\
         console.log(String('color' in cs) + '|' + String('margin-top' in cs));\
         cs.color = 'red'; console.log(cs.color);",
    )
    .expect("computed-style script");
    assert_eq!(
        rt.host().borrow().console,
        vec![
            "rgb(0, 0, 0),16px,block", // color, fontSize (camelCase), getPropertyValue
            "||",                      // unsupported longhands -> ""
            "true|false",              // membership follows supported values
            "rgb(0, 0, 0)",            // read-only: the set was ignored
        ],
    );
}

#[test]
fn get_computed_style_reads_handler_on_boa() {
    get_computed_style_reads_handler::<script_engine_boa::BoaEngine>();
}
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn get_computed_style_reads_handler_on_nova() {
    get_computed_style_reads_handler::<script_engine_nova::NovaEngine>();
}

/// `document.styleSheets` is a stable live list over the host's retained
/// author sheets, with CSSOM mutation errors surfaced as DOMExceptions.
fn stylesheet_cssom_routes_to_handler<E: ScriptEngine>() {
    use std::cell::RefCell;
    use std::rc::Rc;

    #[derive(Clone)]
    struct Stub {
        rules: Rc<RefCell<Vec<Vec<String>>>>,
    }
    impl crate::StyleSheetHandler for Stub {
        fn sheet_count(&self) -> usize {
            self.rules.borrow().len()
        }

        fn rule_count(&self, sheet: usize) -> Option<usize> {
            self.rules.borrow().get(sheet).map(Vec::len)
        }

        fn insert_rule(
            &self,
            sheet: usize,
            rule: &str,
            index: usize,
        ) -> Result<usize, crate::StyleSheetMutationError> {
            let mut sheets = self.rules.borrow_mut();
            let rules = sheets
                .get_mut(sheet)
                .ok_or(crate::StyleSheetMutationError::IndexSize)?;
            if index > rules.len() {
                return Err(crate::StyleSheetMutationError::IndexSize);
            }
            if !rule.trim_start().starts_with('.') {
                return Err(crate::StyleSheetMutationError::Syntax(
                    "expected a style rule".to_string(),
                ));
            }
            rules.insert(index, rule.to_string());
            Ok(index)
        }

        fn delete_rule(
            &self,
            sheet: usize,
            index: usize,
        ) -> Result<(), crate::StyleSheetMutationError> {
            let mut sheets = self.rules.borrow_mut();
            let rules = sheets
                .get_mut(sheet)
                .ok_or(crate::StyleSheetMutationError::IndexSize)?;
            if index >= rules.len() {
                return Err(crate::StyleSheetMutationError::IndexSize);
            }
            rules.remove(index);
            Ok(())
        }
    }

    let rules = Rc::new(RefCell::new(vec![vec![".a { color: red; }".into()]]));
    let mut rt = Runtime::<E>::new().expect("runtime");
    rt.set_stylesheet_handler(Box::new(Stub {
        rules: rules.clone(),
    }));
    rt.eval(
        "var list = document.styleSheets; var sheet = list[0];\
         console.log(String(list === document.styleSheets) + '|' +\
           String(list instanceof StyleSheetList) + '|' + String(sheet instanceof CSSStyleSheet));\
         console.log(list.length + '|' + sheet.cssRules.length + '|' +\
           String(sheet.cssRules instanceof CSSRuleList));\
         console.log(String(sheet.insertRule('.b { color: blue; }', 1)) + '|' +\
           sheet.cssRules.length + '|' + String(sheet === list.item(0)));\
         try { sheet.insertRule('.c {}', 9); } catch (e) { console.log(e.name); }\
         try { sheet.insertRule('not a rule', 2); } catch (e) { console.log(e.name); }\
         console.log(sheet.cssRules.length);\
         sheet.deleteRule(0); console.log(sheet.cssRules.length);",
    )
    .expect("stylesheet CSSOM script");
    assert_eq!(
        rt.host().borrow().console,
        vec![
            "true|true|true",
            "1|1|true",
            "1|2|true",
            "IndexSizeError",
            "SyntaxError",
            "2",
            "1",
        ],
    );
    assert_eq!(rules.borrow()[0], vec![".b { color: blue; }"]);
}

#[test]
fn stylesheet_cssom_routes_to_handler_on_boa() {
    stylesheet_cssom_routes_to_handler::<script_engine_boa::BoaEngine>();
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn stylesheet_cssom_routes_to_handler_on_nova() {
    stylesheet_cssom_routes_to_handler::<script_engine_nova::NovaEngine>();
}

/// `document.cookie` reads the host `CookieProvider` (get) and forwards an
/// assignment (set), the cookie convergence seam (native session store).
fn document_cookie_reads_and_writes_provider<E: ScriptEngine>() {
    use genet_static_dom::StaticDocument;
    use std::cell::RefCell;
    use std::rc::Rc;

    struct Stub {
        written: Rc<RefCell<Vec<String>>>,
    }
    impl crate::CookieProvider for Stub {
        fn get_cookies(&self) -> String {
            "sid=abc; theme=dark".to_string()
        }
        fn set_cookie(&self, cookie: &str) {
            self.written.borrow_mut().push(cookie.to_string());
        }
    }

    let written = Rc::new(RefCell::new(Vec::new()));
    let mut rt = Runtime::<E>::new().expect("runtime");
    rt.load_dom(&StaticDocument::parse("<html><body></body></html>"));
    rt.set_cookie_provider(Box::new(Stub {
        written: written.clone(),
    }));
    rt.eval(
        "console.log(document.cookie);\
         document.cookie = 'new=1; Path=/';\
         console.log(document.cookie);",
    )
    .expect("document.cookie script");

    // The stub's get is constant, so both reads match; the write was forwarded.
    assert_eq!(
        rt.host().borrow().console,
        vec!["sid=abc; theme=dark", "sid=abc; theme=dark"],
    );
    assert_eq!(*written.borrow(), vec!["new=1; Path=/".to_string()]);
}

#[test]
fn document_cookie_reads_and_writes_provider_on_boa() {
    document_cookie_reads_and_writes_provider::<script_engine_boa::BoaEngine>();
}
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn document_cookie_reads_and_writes_provider_on_nova() {
    document_cookie_reads_and_writes_provider::<script_engine_nova::NovaEngine>();
}

#[test]
fn dom_tokenlist_dataset_on_boa() {
    dom_tokenlist_dataset_works::<script_engine_boa::BoaEngine>();
}

/// Repro of the WPT reflection harness's setup (reflection.js getDocument):
/// `document.implementation.createHTMLDocument("")` then query the created doc.
/// This path regressed reflection-* files to ERROR; the assertion diff pinpoints
/// what is non-callable / wrong on the created document.
fn dom_created_doc_queryable<E: ScriptEngine>() {
    use genet_static_dom::StaticDocument;
    let mut rt = Runtime::<E>::new().expect("runtime");
    rt.load_dom(&StaticDocument::parse("<html><body></body></html>"));

    rt.eval(
        "var d = document.implementation.createHTMLDocument('');\
         console.log(typeof d.getElementsByTagName);\
         console.log(typeof d.createElement);\
         var bodies = d.getElementsByTagName('body');\
         console.log(typeof bodies + ',' + String(bodies.length));\
         var p = d.createElement('p'); d.body.appendChild(p);\
         console.log(String(d.getElementsByTagName('p').length));",
    )
    .expect("created-doc query script");

    assert_eq!(
        rt.host().borrow().console,
        vec!["function", "function", "object,1", "1"],
    );
}

/// CharacterData / Text / Comment interface + Node identity, against any
/// backend: the prototype chain + instanceof, data/length, the substring
/// mutators with IndexSizeError, constructors, splitText, isEqualNode,
/// compareDocumentPosition, isConnected, ownerDocument.
fn dom_characterdata_identity_works<E: ScriptEngine>() {
    use genet_static_dom::StaticDocument;
    let mut rt = Runtime::<E>::new().expect("runtime");
    rt.load_dom(&StaticDocument::parse(
        "<html><body><div id='p'></div></body></html>",
    ));

    rt.eval(
        "function thrown(fn){ try { fn(); return 'no-throw'; } catch(e){ return e.name; } }\
         var t = new Text('hello');\
         console.log(String(t instanceof Text) + ',' + String(t instanceof CharacterData) + ',' + String(t instanceof Node));\
         console.log(t.data + ',' + String(t.length) + ',' + t.nodeValue);\
         console.log(String(t.ownerDocument === document));\
         t.appendData(' world'); console.log(t.data);\
         t.insertData(5, ','); console.log(t.data);\
         t.deleteData(0, 6); console.log(t.data);\
         console.log(t.substringData(0, 5));\
         t.replaceData(0, 5, 'WORLD'); console.log(t.data);\
         console.log(thrown(function(){ t.substringData(999, 1); }));\
         var c = new Comment('cm'); console.log(String(c instanceof Comment) + ',' + String(c instanceof CharacterData) + ',' + c.data);\
         var a = document.createElement('div'); a.setAttribute('x','1'); a.textContent='hi';\
         var b = document.createElement('div'); b.setAttribute('x','1'); b.textContent='hi';\
         console.log(String(a.isEqualNode(b)));\
         b.setAttribute('x','2'); console.log(String(a.isEqualNode(b)));\
         var p = document.getElementById('p'); var s1=document.createElement('span'); var s2=document.createElement('span');\
         p.appendChild(s1); p.appendChild(s2);\
         var fol = s1.compareDocumentPosition(s2);\
         console.log(String(!!(fol & Node.DOCUMENT_POSITION_FOLLOWING)) + ',' + String(!!(s2.compareDocumentPosition(s1) & Node.DOCUMENT_POSITION_PRECEDING)));\
         console.log(String(s1.isConnected) + ',' + String(a.isConnected));",
    )
    .expect("characterdata/identity script");

    assert_eq!(
        rt.host().borrow().console,
        vec![
            "true,true,true", // Text instanceof chain
            "hello,5,hello",  // data, length, nodeValue
            "true",           // ownerDocument === document
            "hello world",    // appendData
            "hello, world",   // insertData at 5
            " world",         // deleteData first 6
            " worl",          // substringData(0,5) — read-only, data stays " world"
            "WORLDd",         // replaceData(0,5,'WORLD') over " world" keeps the 6th char
            "IndexSizeError", // substringData out of range
            "true,true,cm",   // Comment instanceof + data
            "true",           // isEqualNode: identical
            "false",          // isEqualNode: differing attr
            "true,true",      // compareDocumentPosition FOLLOWING / PRECEDING
            "true,false",     // isConnected: in-tree vs detached
        ],
    );
}

#[test]
fn dom_created_doc_queryable_on_boa() {
    dom_created_doc_queryable::<script_engine_boa::BoaEngine>();
}

#[test]
fn dom_characterdata_identity_on_boa() {
    dom_characterdata_identity_works::<script_engine_boa::BoaEngine>();
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn dom_characterdata_identity_on_nova() {
    dom_characterdata_identity_works::<script_engine_nova::NovaEngine>();
}

/// DocumentFragment + cloneNode, against any backend: a fragment is nodeType 11
/// and an `instanceof DocumentFragment`; it holds children and is queryable;
/// shallow vs deep cloneNode copy element attributes and (deep) the subtree.
fn dom_fragment_clone_works<E: ScriptEngine>() {
    use genet_static_dom::StaticDocument;
    let mut rt = Runtime::<E>::new().expect("runtime");
    rt.load_dom(&StaticDocument::parse("<html><body></body></html>"));

    rt.eval(
        "var f = document.createDocumentFragment();\
         console.log(String(f.nodeType) + ',' + String(f instanceof DocumentFragment) + ',' + String(f instanceof Node));\
         var d = document.createElement('div'); d.id = 'x'; d.setAttribute('class','c'); f.appendChild(d);\
         d.appendChild(document.createElement('span'));\
         console.log(String(f.childNodes.length) + ',' + (f.getElementById ? (f.getElementById('x') ? 'found' : 'miss') : 'no-gebid'));\
         console.log(String(f.querySelector('span') !== null));\
         var shallow = d.cloneNode(false);\
         console.log(shallow.tagName + ',' + shallow.id + ',' + shallow.getAttribute('class') + ',' + String(shallow.childNodes.length));\
         var deep = d.cloneNode(true);\
         console.log(String(deep.childNodes.length) + ',' + deep.firstChild.tagName);\
         console.log(String(new DocumentFragment() instanceof DocumentFragment));",
    )
    .expect("fragment/clone script");

    assert_eq!(
        rt.host().borrow().console,
        vec![
            "11,true,true", // fragment nodeType + instanceof chain
            "1,found",      // one child; getElementById scoped to the fragment
            "true",         // querySelector('span') finds the nested element
            "DIV,x,c,0",    // shallow clone: tag/id/class copied, no children
            "1,SPAN",       // deep clone: child subtree copied
            "true",         // new DocumentFragment()
        ],
    );
}

#[test]
fn dom_fragment_clone_on_boa() {
    dom_fragment_clone_works::<script_engine_boa::BoaEngine>();
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn dom_fragment_clone_on_nova() {
    dom_fragment_clone_works::<script_engine_nova::NovaEngine>();
}

#[test]
fn dom_implementation_on_boa() {
    dom_implementation_works::<script_engine_boa::BoaEngine>();
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn dom_implementation_on_nova() {
    dom_implementation_works::<script_engine_nova::NovaEngine>();
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn dom_tokenlist_dataset_on_nova() {
    dom_tokenlist_dataset_works::<script_engine_nova::NovaEngine>();
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn dom_collections_on_nova() {
    dom_collections_works::<script_engine_nova::NovaEngine>();
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn dom_url_reflection_on_nova() {
    dom_url_reflection_works::<script_engine_nova::NovaEngine>();
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn proxy_capability_on_nova() {
    proxy_capability::<script_engine_nova::NovaEngine>();
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn dom_traversal_on_nova() {
    dom_traversal_works::<script_engine_nova::NovaEngine>();
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn dom_element_surface_on_nova() {
    dom_element_surface_works::<script_engine_nova::NovaEngine>();
}

#[test]
fn load_dom_on_boa() {
    load_dom_works::<script_engine_boa::BoaEngine>();
}

#[test]
fn load_dom_finds_self_closing_input_on_boa() {
    load_dom_finds_self_closing_input::<script_engine_boa::BoaEngine>();
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn load_dom_on_nova() {
    load_dom_works::<script_engine_nova::NovaEngine>();
}

/// Node-level EventTarget with tree propagation, against any backend: a
/// bubbling event fires on the target then ancestors (with `target` /
/// `currentTarget` set); a non-bubbling event does not reach ancestors;
/// `stopPropagation` halts the climb; `stopImmediatePropagation` halts the
/// current node's remaining listeners and later nodes.
///
/// This is the **JS column** of the event-dispatch conformance table shared
/// with the native dispatcher. Its twin — the same scenarios over
/// `GenetAppRunner::dispatch_click` — is `xilem-serval`'s
/// `stop_propagation_halts_the_bubble_walk` / `prevent_default_is_visible_to_the_caller`.
/// Both must satisfy one contract; see
/// `docs/2026-06-01_event_model_convergence_plan.md`. Change one, mirror the other.
fn dom_node_events_work<E: ScriptEngine>() {
    let mut rt = Runtime::<E>::new().expect("runtime");

    rt.eval(
        "var parent = document.createElement('div');\
         var child = document.createElement('span');\
         parent.appendChild(child);\
         document.appendChild(parent);\
         child.addEventListener('ping', function(e){ console.log('child:' + e.target.tagName); });\
         parent.addEventListener('ping', function(e){ console.log('parent:' + e.currentTarget.tagName); });\
         child.dispatchEvent(new Event('ping', { bubbles: true }));\
         parent.addEventListener('solo', function(){ console.log('solo-bubbled-SHOULD-NOT'); });\
         child.dispatchEvent(new Event('solo'));\
         child.addEventListener('stop', function(e){ e.stopPropagation(); console.log('child-stop'); });\
         parent.addEventListener('stop', function(){ console.log('parent-stop-SHOULD-NOT'); });\
         child.dispatchEvent(new Event('stop', { bubbles: true }));\
         child.addEventListener('imm', function(e){ e.stopImmediatePropagation(); console.log('imm-1'); });\
         child.addEventListener('imm', function(){ console.log('imm-2-SHOULD-NOT'); });\
         parent.addEventListener('imm', function(){ console.log('imm-parent-SHOULD-NOT'); });\
         child.dispatchEvent(new Event('imm', { bubbles: true }));\
         child.addEventListener('once', function(){ console.log('once-fired'); }, { once: true });\
         child.dispatchEvent(new Event('once'));\
         child.dispatchEvent(new Event('once'));\
         var le = document.createEvent('Event');\
         child.addEventListener('legacy', function(e){ console.log('legacy:' + e.type + ':' + e.bubbles); });\
         le.initEvent('legacy', true, true);\
         child.dispatchEvent(le);\
         child.addEventListener('pasv', function(e){ e.preventDefault(); }, { passive: true });\
         var pe = new Event('pasv', { cancelable: true });\
         var notCanceled = child.dispatchEvent(pe);\
         console.log('passive-noop:' + (notCanceled && !pe.defaultPrevented));\
         child.addEventListener('mouse', function(e){ var path = e.composedPath(); console.log('mouse:' + (e instanceof MouseEvent) + ':' + e.clientX + ':' + path[0].tagName + ':' + path[1].tagName + ':' + (path[path.length - 1] === window)); });\
         child.dispatchEvent(new MouseEvent('mouse', { bubbles: true, cancelable: true, clientX: 7, clientY: 9, button: 1 }));\
         child.addEventListener('wheel', function(e){ e.preventDefault(); }, { passive: true });\
         var wheel = new MouseEvent('wheel', { bubbles: true, cancelable: true });\
         console.log('passive-wheel:' + (child.dispatchEvent(wheel) && !wheel.defaultPrevented));",
    )
    .expect("events script");

    // ping bubbles child→parent; solo does not reach parent (no bubble); stop is
    // halted at the child; stopImmediatePropagation halts the child's *second*
    // listener (imm-2) AND the bubble to parent (imm-parent), firing only imm-1;
    // a `once` listener fires on the first dispatch only (second is a no-op); a
    // createEvent()+initEvent() event dispatches with the initialized type/bubbles;
    // a {passive:true} listener's preventDefault() is ignored (dispatchEvent
    // returns true, defaultPrevented stays false).
    assert_eq!(
        rt.host().borrow().console,
        vec![
            "child:SPAN",
            "parent:DIV",
            "child-stop",
            "imm-1",
            "once-fired",
            "legacy:legacy:true",
            "passive-noop:true",
            "mouse:true:7:SPAN:DIV:true",
            "passive-wheel:true",
        ]
    );
}

fn document_evaluate_xpath_works<E: ScriptEngine>() {
    use genet_static_dom::StaticDocument;

    let mut rt = Runtime::<E>::new().expect("runtime");
    let src = StaticDocument::parse(
        "<html><body><main id='main'><h1>Title</h1><p class='lede'>Hello <a href='/x'>link</a></p><p>Second</p></main></body></html>",
    );
    rt.load_dom(&src);

    rt.eval(
        "var h = document.evaluate('string(//h1)', document, null, XPathResult.STRING_TYPE, null);\
         console.log('h:' + h.stringValue);\
         var count = document.evaluate('count(//p)', document, null, XPathResult.NUMBER_TYPE, null);\
         console.log('count:' + count.numberValue);\
         var lede = document.evaluate('//p[@class=\"lede\"]', document, null, XPathResult.FIRST_ORDERED_NODE_TYPE, null);\
         console.log('node:' + lede.singleNodeValue.tagName + ':' + lede.singleNodeValue.textContent);\
         var iter = document.evaluate('//a', document, null, XPathResult.ORDERED_NODE_ITERATOR_TYPE, null);\
         console.log('iter:' + iter.iterateNext().tagName + ':' + String(iter.iterateNext()));\
         var main = document.getElementById('main');\
         var link = document.evaluate('//a', document, null, XPathResult.FIRST_ORDERED_NODE_TYPE, null).singleNodeValue;\
         console.log('preceding-main:' + document.evaluate('count(preceding::main)', link, null, XPathResult.NUMBER_TYPE, null).numberValue);\
         console.log('following-main:' + document.evaluate('following::p', main, null, XPathResult.ORDERED_NODE_SNAPSHOT_TYPE, null).snapshotLength);\
         console.log('nodes-as-scalar:' + document.evaluate('//a', document, null, XPathResult.BOOLEAN_TYPE, null).booleanValue + ':' + document.evaluate('//a', document, null, XPathResult.STRING_TYPE, null).stringValue);",
    )
    .expect("xpath script");

    assert_eq!(
        rt.host().borrow().console,
        vec![
            "h:Title",
            "count:2",
            "node:P:Hello link",
            "iter:A:null",
            "preceding-main:0",
            "following-main:0",
            "nodes-as-scalar:true:link",
        ],
    );
}

/// The **native dispatch entry** (`Runtime::dispatch_event`): the host hands a
/// raw NodeId (as a `hit_test` would) and an event type, and the runtime fires
/// the node's listeners with real propagation — the input → event bridge with no
/// script-side `dispatchEvent` call. `preventDefault` in a listener surfaces to
/// the caller as `false` (the host suppresses the default action). Twin of the
/// JS-column `dom_node_events_work`; same contract, different entry point.
fn dispatch_event_fires_a_listener<E: ScriptEngine>() {
    let mut rt = Runtime::<E>::new().expect("runtime");
    rt.eval(
        "document.addEventListener('click', function(){ console.log('clicked'); });\
         document.addEventListener('cancelme', function(e){ e.preventDefault(); });",
    )
    .expect("listener script");

    // The host's target: the document node's raw id (stands in for a hit-test).
    let root = rt.host().borrow().dom.document().raw();

    // A click runs the listener; with no preventDefault the default may proceed.
    let proceed = rt.dispatch_event(root, "click").expect("dispatch click");
    assert!(proceed);
    assert_eq!(rt.host().borrow().console, vec!["clicked"]);

    // A listener calling preventDefault surfaces as `false` to the host.
    let proceed = rt
        .dispatch_event(root, "cancelme")
        .expect("dispatch cancelme");
    assert!(!proceed);
}

#[test]
fn dispatch_event_fires_a_listener_on_boa() {
    dispatch_event_fires_a_listener::<script_engine_boa::BoaEngine>();
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn dispatch_event_fires_a_listener_on_nova() {
    dispatch_event_fires_a_listener::<script_engine_nova::NovaEngine>();
}

#[test]
fn dom_prototype_dispatch_on_boa() {
    dom_prototype_dispatch_works::<script_engine_boa::BoaEngine>();
}

#[test]
fn dom_node_events_on_boa() {
    dom_node_events_work::<script_engine_boa::BoaEngine>();
}

#[test]
fn document_evaluate_xpath_on_boa() {
    document_evaluate_xpath_works::<script_engine_boa::BoaEngine>();
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn dom_node_events_on_nova() {
    dom_node_events_work::<script_engine_nova::NovaEngine>();
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn document_evaluate_xpath_on_nova() {
    document_evaluate_xpath_works::<script_engine_nova::NovaEngine>();
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn dom_prototype_dispatch_on_nova() {
    dom_prototype_dispatch_works::<script_engine_nova::NovaEngine>();
}

#[test]
fn dom_identity_on_boa() {
    dom_identity_works::<script_engine_boa::BoaEngine>();
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn dom_identity_on_nova() {
    dom_identity_works::<script_engine_nova::NovaEngine>();
}

#[test]
fn dom_read_surface_on_boa() {
    dom_read_surface_works::<script_engine_boa::BoaEngine>();
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn dom_construction_on_nova() {
    dom_construction_works::<script_engine_nova::NovaEngine>();
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn dom_read_surface_on_nova() {
    dom_read_surface_works::<script_engine_nova::NovaEngine>();
}

/// DOMException-throwing on bad input (Lane C item 2), against any backend:
/// invalid names on createElement/setAttribute → InvalidCharacterError, NS
/// validation → NamespaceError, hierarchy cycles → HierarchyRequestError, and
/// createElement lowercases in an HTML document.
fn dom_throwing_works<E: ScriptEngine>() {
    use genet_static_dom::StaticDocument;
    let mut rt = Runtime::<E>::new().expect("runtime");
    rt.load_dom(&StaticDocument::parse(
        "<html><body><div id='p'></div></body></html>",
    ));

    rt.eval(
        "function thrown(fn){ try { fn(); return 'no-throw'; } catch(e){ return (e && e.name) || 'err'; } }\
         console.log(thrown(function(){ document.createElement('1foo'); }));\
         console.log(thrown(function(){ document.createElement('f<oo'); }));\
         console.log(document.createElement('DIV').localName);\
         console.log(document.createElement(':foo').localName);\
         console.log(thrown(function(){ document.getElementById('p').setAttribute('a b', 'x'); }));\
         console.log(thrown(function(){ document.createElementNS(null, 'p:q'); }));\
         console.log(thrown(function(){ document.createElementNS('urn:x', 'a:b:c'); }));\
         console.log(document.createElementNS('urn:x', 'a:b').tagName);\
         var p = document.getElementById('p'); var c = document.createElement('span'); p.appendChild(c);\
         console.log(thrown(function(){ c.appendChild(p); }));\
         console.log(thrown(function(){ p.appendChild(p); }));",
    )
    .expect("throwing script");

    assert_eq!(
        rt.host().borrow().console,
        vec![
            "InvalidCharacterError", // createElement('1foo')
            "InvalidCharacterError", // createElement('f<oo')
            "div",                   // createElement('DIV') lowercases
            ":foo",                  // ':foo' is a valid Name, not lowercased away
            "InvalidCharacterError", // setAttribute('a b', ...)
            "NamespaceError",        // createElementNS(null, 'p:q') — prefix needs ns
            "InvalidCharacterError", // 'a:b:c' — malformed qualified name
            "a:b",                   // valid NS element, tagName not upper (non-HTML ns)
            "HierarchyRequestError", // c.appendChild(p) — p is ancestor of c
            "HierarchyRequestError", // p.appendChild(p) — self
        ],
    );
}

#[test]
fn dom_throwing_on_boa() {
    dom_throwing_works::<script_engine_boa::BoaEngine>();
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn dom_throwing_on_nova() {
    dom_throwing_works::<script_engine_nova::NovaEngine>();
}
