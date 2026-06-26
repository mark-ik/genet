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
    // serval-layout: setAttribute, two appendChilds, textContent → 4 mutations.
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
    assert_eq!(rt.host().borrow().console, vec!["true", "true", "false", "true"]);
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
    assert_eq!(rt.host().borrow().console, vec!["true", "true", "true", "true", "true"]);
}

/// `load_dom`, against any backend: a parsed source document becomes the live
/// DOM, so script sees `document.body`, `getElementById`, and tag queries over
/// the pre-existing tree.
fn load_dom_works<E: ScriptEngine>() {
    use serval_static_dom::StaticDocument;
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
         console.log(String(document.getElementsByTagName('p').length));",
    )
    .expect("query script");

    assert_eq!(rt.host().borrow().console, vec!["BODY", "HTML", "DIV", "1"]);
}

/// Regression probe for the dom/events corpus: a self-closing
/// `<input id=target type=hidden/>` in the loaded body must be queryable by
/// `getElementById` and usable as an EventTarget (the shape of
/// `Event-defaultPrevented-after-dispatch`, which failed with "cannot convert
/// null to object" when the input wasn't found).
fn load_dom_finds_self_closing_input<E: ScriptEngine>() {
    use serval_static_dom::StaticDocument;
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
    use serval_static_dom::StaticDocument;
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
    use serval_static_dom::StaticDocument;
    let mut rt = Runtime::<E>::new().expect("runtime");
    rt.load_dom(&StaticDocument::parse("<html><body><div id=d>x</div></body></html>"));
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
        "cancelBubble before dispatch must not reach window (got {})", log[0]
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
    use serval_static_dom::StaticDocument;
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
            "true",        // div instanceof Element
            "1",           // nodeType ELEMENT_NODE
            "a,x y",       // id, className
            "true,false",  // hasAttribute
            "x z,true,2",  // className after add('z')/remove('y'); classList has x; length 2
            "true",        // toggleAttribute added 'hidden'
            "2",           // .x matches div + p
            "hi",          // div > p textContent
            "1",           // div's span descendants
            "true",        // p matches .x
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
    use serval_static_dom::StaticDocument;
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
            "3",            // childNodes: text + span#a + span#b
            "#text,text",   // firstChild nodeName/nodeValue
            "2",            // childElementCount (two spans)
            "a,b",          // first/last element child ids
            "b,null",       // a.nextElementSibling=b, previousElementSibling=null
            "a,c,b",        // after insertBefore(c, b)
            "c,b",          // after removeChild(a)
            "c,d,b",        // after c.after(d)
            "c,b",          // after d.remove()
            "true,false",   // contains c (yes), a (removed, no)
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
    use serval_static_dom::StaticDocument;
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
            "string,boolean,number",       // typeof reflected attrs
            "T,T",                          // title set reflects to attribute
            "true,true",                    // hidden boolean reflects
            "false",                        // hidden cleared
            "3",                            // tabIndex long roundtrip
            "a,http://www.w3.org/1999/xhtml,A", // localName/namespaceURI/tagName (HTML upper)
            "rect,http://www.w3.org/2000/svg,svg,svg:rect", // createElementNS: localName=rect, tagName=qualified (prefix kept, not upper-cased)
            "Hi there",                     // document.title whitespace-collapsed
            "object,1",                     // NodeFilter present
            "a",                            // tree walker over body finds the <a>
        ],
    );
}

#[test]
fn dom_traversal_on_boa() {
    dom_traversal_works::<script_engine_boa::BoaEngine>();
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
    assert_eq!(rt.host().borrow().console, vec!["zero", "1", "true", "0,length"]);
}

/// Live HTMLCollection / NodeList exotic semantics, against any backend:
/// liveness, item/namedItem, indexed + named access, Symbol.iterator, the
/// HTMLCollection-lacks-forEach / NodeList-has-forEach distinction, and
/// getOwnPropertyNames order (indices then deduped non-empty id/name).
fn dom_collections_works<E: ScriptEngine>() {
    use serval_static_dom::StaticDocument;
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
            "2,x,y",          // length, [0].id, item(1) name
            "x,null",         // namedItem hit / miss
            "y",              // named access c['y']
            "true,false,false", // Symbol.iterator yes; forEach/values no (HTMLCollection)
            "0,1,x,y",        // getOwnPropertyNames: indices 0,1 then names x,y
            "3",              // live: after appending a third span
            "3,function,false", // childNodes NodeList: 3 kids, has forEach, no namedItem
            "SPAN,SPAN,SPAN", // forEach over the NodeList
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
    use serval_static_dom::StaticDocument;
    let mut rt = Runtime::<E>::new().expect("runtime");
    rt.load_dom(&StaticDocument::parse("<html><body><a id='a' class='x y'></a></body></html>"));

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
            "[object DOMTokenList]",     // brand
            "function,function,true",    // values, forEach, Symbol.iterator
            "x y,2,x,y",                 // value, length, [0], [1]
            "true,z y",                  // replace('x','z') -> 'z y'
            "z,y",                       // forEach over tokens
            "2,true",                    // relList from rel='next prev'
            "v",                         // dataset.fooBar -> data-foo-bar
            "w",                         // data-baz -> dataset.baz
            "true,false",                // 'fooBar' in dataset, 'nope' not
            "baz,fooBar",                // Object.keys(dataset) (sorted)
            "false",                     // delete dataset.baz removed the attr
        ],
    );
}

/// URL reflected IDL attributes (`href`, `src`, …): the getter resolves the
/// content attribute against the document base URL, the setter stores the raw
/// string, and an absent attribute reflects as the empty string.
fn dom_url_reflection_works<E: ScriptEngine>() {
    use serval_static_dom::StaticDocument;
    let mut rt = Runtime::<E>::new().expect("runtime");
    rt.set_base_url("http://example.com/dir/page.html").expect("base url");
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
    use serval_static_dom::StaticDocument;
    let mut rt = Runtime::<E>::new().expect("runtime");
    rt.load_dom(&StaticDocument::parse("<html><body><p id='main'></p></body></html>"));

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
            "true",                  // hasFeature always true
            "Hello,HTML,BODY",       // createHTMLDocument: title, <html>, <body>
            "sub",                   // getElementById scoped to the created doc
            "null",                  // primary document does NOT see the created doc's #sub
            "main-here",             // primary document still finds its own #main
            "root,urn:ns",           // createDocument: root element + namespace
        ],
    );
}

/// `element.style` is a CSSStyleDeclaration over the inline `style` attribute:
/// getPropertyValue / camelCase get + set / setProperty / removeProperty /
/// length / item / cssText / `in`, all writing back to the attribute.
fn element_style_inline_cssom<E: ScriptEngine>() {
    use serval_static_dom::StaticDocument;
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
         console.log(s.cssText + ',' + s.color);",
    )
    .expect("style script");
    assert_eq!(
        rt.host().borrow().console,
        vec![
            "red,12px,12px",   // .color, .fontSize (camelCase), getPropertyValue
            "2,color,font-size", // length, item(0), item(1)
            "color: blue; font-size: 12px; margin-top: 4px; font-weight: bold;",
            "12px,true,false", // removeProperty returns old; 'color' in / 'display' in
            "padding: 1px; color: green;,green", // cssText set + read; .color
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

/// `getComputedStyle(el)` reads through the host `ComputedStyleHandler` seam:
/// supported longhands resolve (camelCase + getPropertyValue), unsupported
/// ones yield "", and the declaration is read-only.
fn get_computed_style_reads_handler<E: ScriptEngine>() {
    use serval_static_dom::StaticDocument;
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
    rt.load_dom(&StaticDocument::parse("<html><body><div id='d'></div></body></html>"));
    rt.set_computed_style_handler(Box::new(Stub));
    rt.eval(
        "var cs = getComputedStyle(document.getElementById('d'));\
         console.log(cs.color + ',' + cs.fontSize + ',' + cs.getPropertyValue('display'));\
         console.log(cs.getPropertyValue('margin-top') + '|' + cs.marginTop + '|' + cs.bogus);\
         cs.color = 'red'; console.log(cs.color);",
    )
    .expect("computed-style script");
    assert_eq!(
        rt.host().borrow().console,
        vec![
            "rgb(0, 0, 0),16px,block", // color, fontSize (camelCase), getPropertyValue
            "||",                       // unsupported longhands -> ""
            "rgb(0, 0, 0)",             // read-only: the set was ignored
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

/// `document.cookie` reads the host `CookieProvider` (get) and forwards an
/// assignment (set), the cookie convergence seam (native session store).
fn document_cookie_reads_and_writes_provider<E: ScriptEngine>() {
    use serval_static_dom::StaticDocument;
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
    rt.set_cookie_provider(Box::new(Stub { written: written.clone() }));
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
    use serval_static_dom::StaticDocument;
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
    use serval_static_dom::StaticDocument;
    let mut rt = Runtime::<E>::new().expect("runtime");
    rt.load_dom(&StaticDocument::parse("<html><body><div id='p'></div></body></html>"));

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
            "true,true,true",   // Text instanceof chain
            "hello,5,hello",    // data, length, nodeValue
            "true",             // ownerDocument === document
            "hello world",      // appendData
            "hello, world",     // insertData at 5
            " world",           // deleteData first 6
            " worl",            // substringData(0,5) — read-only, data stays " world"
            "WORLDd",           // replaceData(0,5,'WORLD') over " world" keeps the 6th char
            "IndexSizeError",   // substringData out of range
            "true,true,cm",     // Comment instanceof + data
            "true",             // isEqualNode: identical
            "false",            // isEqualNode: differing attr
            "true,true",        // compareDocumentPosition FOLLOWING / PRECEDING
            "true,false",       // isConnected: in-tree vs detached
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
    use serval_static_dom::StaticDocument;
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
            "11,true,true",   // fragment nodeType + instanceof chain
            "1,found",        // one child; getElementById scoped to the fragment
            "true",           // querySelector('span') finds the nested element
            "DIV,x,c,0",      // shallow clone: tag/id/class copied, no children
            "1,SPAN",         // deep clone: child subtree copied
            "true",           // new DocumentFragment()
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
/// `ServalAppRunner::dispatch_click` — is `xilem-serval`'s
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
         console.log('passive-noop:' + (notCanceled && !pe.defaultPrevented));",
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
        ]
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
    let proceed = rt.dispatch_event(root, "cancelme").expect("dispatch cancelme");
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

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn dom_node_events_on_nova() {
    dom_node_events_work::<script_engine_nova::NovaEngine>();
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
    use serval_static_dom::StaticDocument;
    let mut rt = Runtime::<E>::new().expect("runtime");
    rt.load_dom(&StaticDocument::parse("<html><body><div id='p'></div></body></html>"));

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
            "InvalidCharacterError",  // createElement('1foo')
            "InvalidCharacterError",  // createElement('f<oo')
            "div",                    // createElement('DIV') lowercases
            ":foo",                   // ':foo' is a valid Name, not lowercased away
            "InvalidCharacterError",  // setAttribute('a b', ...)
            "NamespaceError",         // createElementNS(null, 'p:q') — prefix needs ns
            "InvalidCharacterError",  // 'a:b:c' — malformed qualified name
            "a:b",                    // valid NS element, tagName not upper (non-HTML ns)
            "HierarchyRequestError",  // c.appendChild(p) — p is ancestor of c
            "HierarchyRequestError",  // p.appendChild(p) — self
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
