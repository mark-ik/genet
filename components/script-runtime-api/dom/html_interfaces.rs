//! Declarative HTML interface metadata consumed by the DOM bootstrap.
//!
//! This is the first cut of the interface table: Rust owns the constructor/tag/
//! reflected-attribute data, while the shared JS bootstrap still owns the small
//! amount of algorithm glue that calls native sinks.

struct HtmlInterface {
    name: &'static str,
    parent: &'static str,
    tags: &'static [&'static str],
    reflected: &'static [ReflectedAttribute],
    members: &'static [&'static str],
}

struct ReflectedAttribute {
    idl: &'static str,
    kind: &'static str,
    attr: Option<&'static str>,
    keywords: &'static [&'static str],
    missing: Option<&'static str>,
}

macro_rules! attr {
    ($idl:literal, $kind:literal) => {
        ReflectedAttribute {
            idl: $idl,
            kind: $kind,
            attr: None,
            keywords: &[],
            missing: None,
        }
    };
    ($idl:literal, $kind:literal, attr = $content:literal) => {
        ReflectedAttribute {
            idl: $idl,
            kind: $kind,
            attr: Some($content),
            keywords: &[],
            missing: None,
        }
    };
    ($idl:literal, $kind:literal, missing = $missing:literal) => {
        ReflectedAttribute {
            idl: $idl,
            kind: $kind,
            attr: None,
            keywords: &[],
            missing: Some($missing),
        }
    };
    ($idl:literal, $kind:literal, attr = $content:literal, missing = $missing:literal) => {
        ReflectedAttribute {
            idl: $idl,
            kind: $kind,
            attr: Some($content),
            keywords: &[],
            missing: Some($missing),
        }
    };
    ($idl:literal, enum [$($keyword:literal),* $(,)?]) => {
        ReflectedAttribute {
            idl: $idl,
            kind: "e",
            attr: None,
            keywords: &[$($keyword),*],
            missing: None,
        }
    };
    ($idl:literal, enum [$($keyword:literal),* $(,)?], attr = $content:literal) => {
        ReflectedAttribute {
            idl: $idl,
            kind: "e",
            attr: Some($content),
            keywords: &[$($keyword),*],
            missing: None,
        }
    };
    ($idl:literal, enum [$($keyword:literal),* $(,)?], missing = $missing:literal) => {
        ReflectedAttribute {
            idl: $idl,
            kind: "e",
            attr: None,
            keywords: &[$($keyword),*],
            missing: Some($missing),
        }
    };
    ($idl:literal, enum [$($keyword:literal),* $(,)?], attr = $content:literal, missing = $missing:literal) => {
        ReflectedAttribute {
            idl: $idl,
            kind: "e",
            attr: Some($content),
            keywords: &[$($keyword),*],
            missing: Some($missing),
        }
    };
}

const HTML_INTERFACES: &[HtmlInterface] = &[
    HtmlInterface {
        name: "HTMLElement",
        parent: "Element",
        tags: &[],
        reflected: &[
            attr!("title", "s"),
            attr!("lang", "s"),
            attr!("accessKey", "s"),
            attr!("autofocus", "b"),
            attr!("hidden", "b"),
            attr!("tabIndex", "l", missing = "-1"),
            attr!("dir", enum ["ltr", "rtl", "auto"]),
            attr!("nonce", "s"),
        ],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLHtmlElement",
        parent: "HTMLElement",
        tags: &["html"],
        reflected: &[attr!("version", "s")],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLHeadElement",
        parent: "HTMLElement",
        tags: &["head"],
        reflected: &[],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLTitleElement",
        parent: "HTMLElement",
        tags: &["title"],
        reflected: &[attr!("text", "tc")],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLBaseElement",
        parent: "HTMLElement",
        tags: &["base"],
        reflected: &[attr!("href", "u"), attr!("target", "s")],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLLinkElement",
        parent: "HTMLElement",
        tags: &["link"],
        reflected: &[
            attr!("href", "u"),
            attr!("crossOrigin", enum ["anonymous", "use-credentials"], attr = "crossorigin"),
            attr!("rel", "s"),
            attr!("relList", "t", attr = "rel"),
            attr!("media", "s"),
            attr!("hreflang", "s"),
            attr!("type", "s"),
            attr!("sizes", "s"),
            attr!("integrity", "s"),
            attr!("referrerPolicy", enum [
                "",
                "no-referrer",
                "no-referrer-when-downgrade",
                "same-origin",
                "origin",
                "strict-origin",
                "origin-when-cross-origin",
                "strict-origin-when-cross-origin",
                "unsafe-url"
            ], attr = "referrerpolicy"),
        ],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLMetaElement",
        parent: "HTMLElement",
        tags: &["meta"],
        reflected: &[
            attr!("name", "s"),
            attr!("httpEquiv", "s", attr = "http-equiv"),
            attr!("content", "s"),
            attr!("media", "s"),
            attr!("scheme", "s"),
        ],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLStyleElement",
        parent: "HTMLElement",
        tags: &["style"],
        reflected: &[attr!("media", "s"), attr!("type", "s")],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLBodyElement",
        parent: "HTMLElement",
        tags: &["body"],
        reflected: &[
            attr!("aLink", "s", attr = "alink"),
            attr!("background", "s"),
            attr!("bgColor", "s", attr = "bgcolor"),
            attr!("link", "s"),
            attr!("text", "s"),
            attr!("vLink", "s", attr = "vlink"),
        ],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLHeadingElement",
        parent: "HTMLElement",
        tags: &["h1", "h2", "h3", "h4", "h5", "h6"],
        reflected: &[attr!("align", "s")],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLParagraphElement",
        parent: "HTMLElement",
        tags: &["p"],
        reflected: &[attr!("align", "s")],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLHRElement",
        parent: "HTMLElement",
        tags: &["hr"],
        reflected: &[
            attr!("align", "s"),
            attr!("color", "s"),
            attr!("noShade", "b", attr = "noshade"),
            attr!("size", "s"),
            attr!("width", "s"),
        ],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLPreElement",
        parent: "HTMLElement",
        tags: &["pre"],
        reflected: &[attr!("width", "ul", missing = "0")],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLQuoteElement",
        parent: "HTMLElement",
        tags: &["blockquote", "q"],
        reflected: &[attr!("cite", "u")],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLOListElement",
        parent: "HTMLElement",
        tags: &["ol"],
        reflected: &[
            attr!("reversed", "b"),
            attr!("start", "l", missing = "1"),
            attr!("type", "s"),
            attr!("compact", "b"),
        ],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLUListElement",
        parent: "HTMLElement",
        tags: &["ul"],
        reflected: &[attr!("compact", "b"), attr!("type", "s")],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLMenuElement",
        parent: "HTMLElement",
        tags: &["menu"],
        reflected: &[attr!("compact", "b")],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLLIElement",
        parent: "HTMLElement",
        tags: &["li"],
        reflected: &[attr!("value", "l", missing = "0"), attr!("type", "s")],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLDListElement",
        parent: "HTMLElement",
        tags: &["dl"],
        reflected: &[attr!("compact", "b")],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLDivElement",
        parent: "HTMLElement",
        tags: &["div"],
        reflected: &[attr!("align", "s")],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLSpanElement",
        parent: "HTMLElement",
        tags: &["span"],
        reflected: &[],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLBRElement",
        parent: "HTMLElement",
        tags: &["br"],
        reflected: &[attr!("clear", "s")],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLAnchorElement",
        parent: "HTMLElement",
        tags: &["a"],
        reflected: &[
            attr!("href", "u"),
            attr!("target", "s"),
            attr!("download", "s"),
            attr!("ping", "s"),
            attr!("rel", "s"),
            attr!("relList", "t", attr = "rel"),
            attr!("hreflang", "s"),
            attr!("type", "s"),
            attr!("text", "tc"),
            attr!("referrerPolicy", enum [
                "",
                "no-referrer",
                "no-referrer-when-downgrade",
                "same-origin",
                "origin",
                "strict-origin",
                "origin-when-cross-origin",
                "strict-origin-when-cross-origin",
                "unsafe-url"
            ], attr = "referrerpolicy"),
        ],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLAreaElement",
        parent: "HTMLElement",
        tags: &["area"],
        reflected: &[
            attr!("alt", "s"),
            attr!("coords", "s"),
            attr!("shape", "s"),
            attr!("target", "s"),
            attr!("download", "s"),
            attr!("ping", "s"),
            attr!("rel", "s"),
            attr!("relList", "t", attr = "rel"),
            attr!("href", "u"),
            attr!("noHref", "b", attr = "nohref"),
            attr!("referrerPolicy", enum [
                "",
                "no-referrer",
                "no-referrer-when-downgrade",
                "same-origin",
                "origin",
                "strict-origin",
                "origin-when-cross-origin",
                "strict-origin-when-cross-origin",
                "unsafe-url"
            ], attr = "referrerpolicy"),
        ],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLImageElement",
        parent: "HTMLElement",
        tags: &["img"],
        reflected: &[
            attr!("alt", "s"),
            attr!("src", "u"),
            attr!("srcset", "s"),
            attr!("sizes", "s"),
            attr!("crossOrigin", enum ["anonymous", "use-credentials"], attr = "crossorigin"),
            attr!("useMap", "s", attr = "usemap"),
            attr!("isMap", "b", attr = "ismap"),
            attr!("decoding", enum ["async", "sync", "auto"]),
            attr!("loading", enum ["lazy", "eager"]),
            attr!("referrerPolicy", enum [
                "",
                "no-referrer",
                "no-referrer-when-downgrade",
                "same-origin",
                "origin",
                "strict-origin",
                "origin-when-cross-origin",
                "strict-origin-when-cross-origin",
                "unsafe-url"
            ], attr = "referrerpolicy"),
            attr!("name", "s"),
            attr!("align", "s"),
            attr!("border", "s"),
        ],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLCanvasElement",
        parent: "HTMLElement",
        tags: &["canvas"],
        reflected: &[
            attr!("width", "ul", missing = "300"),
            attr!("height", "ul", missing = "150"),
        ],
        members: &["canvas_context"],
    },
    HtmlInterface {
        name: "HTMLMapElement",
        parent: "HTMLElement",
        tags: &["map"],
        reflected: &[attr!("name", "s")],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLTableElement",
        parent: "HTMLElement",
        tags: &["table"],
        reflected: &[
            attr!("align", "s"),
            attr!("border", "s"),
            attr!("frame", "s"),
            attr!("rules", "s"),
            attr!("summary", "s"),
            attr!("width", "s"),
            attr!("bgColor", "s", attr = "bgcolor"),
            attr!("cellPadding", "s", attr = "cellpadding"),
            attr!("cellSpacing", "s", attr = "cellspacing"),
        ],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLTableCaptionElement",
        parent: "HTMLElement",
        tags: &["caption"],
        reflected: &[attr!("align", "s")],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLTableColElement",
        parent: "HTMLElement",
        tags: &["col", "colgroup"],
        reflected: &[
            attr!("align", "s"),
            attr!("ch", "s", attr = "char"),
            attr!("chOff", "s", attr = "charoff"),
            attr!("span", "ul", missing = "1"),
            attr!("vAlign", "s", attr = "valign"),
            attr!("width", "s"),
        ],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLTableSectionElement",
        parent: "HTMLElement",
        tags: &["thead", "tbody", "tfoot"],
        reflected: &[
            attr!("align", "s"),
            attr!("ch", "s", attr = "char"),
            attr!("chOff", "s", attr = "charoff"),
            attr!("vAlign", "s", attr = "valign"),
        ],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLTableRowElement",
        parent: "HTMLElement",
        tags: &["tr"],
        reflected: &[
            attr!("align", "s"),
            attr!("bgColor", "s", attr = "bgcolor"),
            attr!("ch", "s", attr = "char"),
            attr!("chOff", "s", attr = "charoff"),
            attr!("vAlign", "s", attr = "valign"),
        ],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLTableCellElement",
        parent: "HTMLElement",
        tags: &["td", "th"],
        reflected: &[
            attr!("abbr", "s"),
            attr!("align", "s"),
            attr!("axis", "s"),
            attr!("bgColor", "s", attr = "bgcolor"),
            attr!("ch", "s", attr = "char"),
            attr!("chOff", "s", attr = "charoff"),
            attr!("colSpan", "ul", attr = "colspan", missing = "1"),
            attr!("headers", "s"),
            attr!("height", "s"),
            attr!("noWrap", "b", attr = "nowrap"),
            attr!("rowSpan", "ul", attr = "rowspan", missing = "1"),
            attr!("scope", enum ["row", "col", "rowgroup", "colgroup"]),
            attr!("vAlign", "s", attr = "valign"),
            attr!("width", "s"),
        ],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLFormElement",
        parent: "HTMLElement",
        tags: &["form"],
        reflected: &[
            attr!("acceptCharset", "s", attr = "accept-charset"),
            attr!("action", "u"),
            attr!("autocomplete", enum ["on", "off"]),
            attr!("enctype", enum [
                "application/x-www-form-urlencoded",
                "multipart/form-data",
                "text/plain"
            ]),
            attr!("encoding", enum [
                "application/x-www-form-urlencoded",
                "multipart/form-data",
                "text/plain"
            ]),
            attr!("method", enum ["get", "post", "dialog"]),
            attr!("name", "s"),
            attr!("noValidate", "b", attr = "novalidate"),
            attr!("target", "s"),
            attr!("rel", "s"),
            attr!("relList", "t", attr = "rel"),
        ],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLLabelElement",
        parent: "HTMLElement",
        tags: &["label"],
        reflected: &[attr!("htmlFor", "s", attr = "for")],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLInputElement",
        parent: "HTMLElement",
        tags: &["input"],
        reflected: &[
            attr!("accept", "s"),
            attr!("alt", "s"),
            attr!("autocomplete", enum ["on", "off"]),
            attr!("defaultChecked", "b", attr = "checked"),
            attr!("defaultValue", "s", attr = "value"),
            attr!("dirName", "s", attr = "dirname"),
            attr!("disabled", "b"),
            attr!("formAction", "u", attr = "formaction"),
            attr!("formEnctype", enum [
                "application/x-www-form-urlencoded",
                "multipart/form-data",
                "text/plain"
            ], attr = "formenctype"),
            attr!("formNoValidate", "b", attr = "formnovalidate"),
            attr!("formTarget", "s", attr = "formtarget"),
            attr!("multiple", "b"),
            attr!("name", "s"),
            attr!("pattern", "s"),
            attr!("placeholder", "s"),
            attr!("readOnly", "b", attr = "readonly"),
            attr!("required", "b"),
            attr!("src", "u"),
            attr!("step", "s"),
            attr!("type", "s"),
            attr!("useMap", "s", attr = "usemap"),
            attr!("value", "s"),
        ],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLButtonElement",
        parent: "HTMLElement",
        tags: &["button"],
        reflected: &[
            attr!("disabled", "b"),
            attr!("formAction", "u", attr = "formaction"),
            attr!("formEnctype", enum [
                "application/x-www-form-urlencoded",
                "multipart/form-data",
                "text/plain"
            ], attr = "formenctype"),
            attr!("formNoValidate", "b", attr = "formnovalidate"),
            attr!("formTarget", "s", attr = "formtarget"),
            attr!("name", "s"),
            attr!("type", enum ["submit", "reset", "button"], missing = "submit"),
            attr!("value", "s"),
        ],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLSelectElement",
        parent: "HTMLElement",
        tags: &["select"],
        reflected: &[
            attr!("autocomplete", enum ["on", "off"]),
            attr!("disabled", "b"),
            attr!("multiple", "b"),
            attr!("name", "s"),
            attr!("required", "b"),
            attr!("size", "ul", missing = "0"),
        ],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLDataListElement",
        parent: "HTMLElement",
        tags: &["datalist"],
        reflected: &[],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLOptGroupElement",
        parent: "HTMLElement",
        tags: &["optgroup"],
        reflected: &[attr!("disabled", "b"), attr!("label", "s")],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLOptionElement",
        parent: "HTMLElement",
        tags: &["option"],
        reflected: &[
            attr!("disabled", "b"),
            attr!("label", "s"),
            attr!("defaultSelected", "b", attr = "selected"),
            attr!("value", "s"),
            attr!("text", "tc"),
        ],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLTextAreaElement",
        parent: "HTMLElement",
        tags: &["textarea"],
        reflected: &[
            attr!("autocomplete", enum ["on", "off"]),
            attr!("defaultValue", "s", attr = "value"),
            attr!("dirName", "s", attr = "dirname"),
            attr!("disabled", "b"),
            attr!("name", "s"),
            attr!("placeholder", "s"),
            attr!("readOnly", "b", attr = "readonly"),
            attr!("required", "b"),
            attr!("wrap", "s"),
            attr!("value", "s"),
        ],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLOutputElement",
        parent: "HTMLElement",
        tags: &["output"],
        reflected: &[
            attr!("htmlFor", "t", attr = "for"),
            attr!("name", "s"),
            attr!("defaultValue", "s", attr = "value"),
            attr!("value", "s"),
        ],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLProgressElement",
        parent: "HTMLElement",
        tags: &["progress"],
        reflected: &[],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLMeterElement",
        parent: "HTMLElement",
        tags: &["meter"],
        reflected: &[],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLFieldSetElement",
        parent: "HTMLElement",
        tags: &["fieldset"],
        reflected: &[attr!("disabled", "b"), attr!("name", "s")],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLLegendElement",
        parent: "HTMLElement",
        tags: &["legend"],
        reflected: &[attr!("align", "s")],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLEmbedElement",
        parent: "HTMLElement",
        tags: &["embed"],
        reflected: &[
            attr!("src", "u"),
            attr!("type", "s"),
            attr!("width", "s"),
            attr!("height", "s"),
            attr!("align", "s"),
            attr!("name", "s"),
        ],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLIFrameElement",
        parent: "HTMLElement",
        tags: &["iframe"],
        reflected: &[
            attr!("src", "u"),
            attr!("srcdoc", "s"),
            attr!("name", "s"),
            attr!("allowFullscreen", "b", attr = "allowfullscreen"),
            attr!("width", "s"),
            attr!("height", "s"),
            attr!("referrerPolicy", enum [
                "",
                "no-referrer",
                "no-referrer-when-downgrade",
                "same-origin",
                "origin",
                "strict-origin",
                "origin-when-cross-origin",
                "strict-origin-when-cross-origin",
                "unsafe-url"
            ], attr = "referrerpolicy"),
            attr!("loading", enum ["lazy", "eager"]),
            attr!("align", "s"),
            attr!("frameBorder", "s", attr = "frameborder"),
            attr!("marginHeight", "s", attr = "marginheight"),
            attr!("marginWidth", "s", attr = "marginwidth"),
            attr!("scrolling", "s"),
        ],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLObjectElement",
        parent: "HTMLElement",
        tags: &["object"],
        reflected: &[
            attr!("data", "u"),
            attr!("type", "s"),
            attr!("name", "s"),
            attr!("useMap", "s", attr = "usemap"),
            attr!("width", "s"),
            attr!("height", "s"),
            attr!("align", "s"),
            attr!("archive", "s"),
            attr!("border", "s"),
            attr!("code", "s"),
            attr!("codeType", "s", attr = "codetype"),
            attr!("declare", "b"),
            attr!("standby", "s"),
        ],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLParamElement",
        parent: "HTMLElement",
        tags: &["param"],
        reflected: &[
            attr!("name", "s"),
            attr!("value", "s"),
            attr!("type", "s"),
            attr!("valueType", "s", attr = "valuetype"),
        ],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLVideoElement",
        parent: "HTMLElement",
        tags: &["video"],
        reflected: &[
            attr!("src", "u"),
            attr!("crossOrigin", enum ["anonymous", "use-credentials"], attr = "crossorigin"),
            attr!("preload", enum ["none", "metadata", "auto"]),
            attr!("autoplay", "b"),
            attr!("loop", "b"),
            attr!("controls", "b"),
            attr!("poster", "u"),
            attr!("playsInline", "b", attr = "playsinline"),
            attr!("width", "ul", missing = "0"),
            attr!("height", "ul", missing = "0"),
        ],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLAudioElement",
        parent: "HTMLElement",
        tags: &["audio"],
        reflected: &[
            attr!("src", "u"),
            attr!("crossOrigin", enum ["anonymous", "use-credentials"], attr = "crossorigin"),
            attr!("preload", enum ["none", "metadata", "auto"]),
            attr!("autoplay", "b"),
            attr!("loop", "b"),
            attr!("controls", "b"),
        ],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLSourceElement",
        parent: "HTMLElement",
        tags: &["source"],
        reflected: &[
            attr!("src", "u"),
            attr!("type", "s"),
            attr!("srcset", "s"),
            attr!("sizes", "s"),
            attr!("media", "s"),
        ],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLTrackElement",
        parent: "HTMLElement",
        tags: &["track"],
        reflected: &[
            attr!("default", "b"),
            attr!("kind", enum ["subtitles", "captions", "descriptions", "chapters", "metadata"]),
            attr!("label", "s"),
            attr!("src", "u"),
            attr!("srclang", "s"),
        ],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLScriptElement",
        parent: "HTMLElement",
        tags: &["script"],
        reflected: &[
            attr!("src", "u"),
            attr!("type", "s"),
            attr!("noModule", "b", attr = "nomodule"),
            attr!("defer", "b"),
            attr!("crossOrigin", enum ["anonymous", "use-credentials"], attr = "crossorigin"),
            attr!("text", "tc"),
            attr!("integrity", "s"),
            attr!("referrerPolicy", enum [
                "",
                "no-referrer",
                "no-referrer-when-downgrade",
                "same-origin",
                "origin",
                "strict-origin",
                "origin-when-cross-origin",
                "strict-origin-when-cross-origin",
                "unsafe-url"
            ], attr = "referrerpolicy"),
        ],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLTemplateElement",
        parent: "HTMLElement",
        tags: &["template"],
        reflected: &[],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLSlotElement",
        parent: "HTMLElement",
        tags: &["slot"],
        reflected: &[attr!("name", "s")],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLDataElement",
        parent: "HTMLElement",
        tags: &["data"],
        reflected: &[attr!("value", "s")],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLTimeElement",
        parent: "HTMLElement",
        tags: &["time"],
        reflected: &[attr!("dateTime", "s", attr = "datetime")],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLModElement",
        parent: "HTMLElement",
        tags: &["del", "ins"],
        reflected: &[
            attr!("cite", "u"),
            attr!("dateTime", "s", attr = "datetime"),
        ],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLDetailsElement",
        parent: "HTMLElement",
        tags: &["details"],
        reflected: &[attr!("open", "b")],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLDialogElement",
        parent: "HTMLElement",
        tags: &["dialog"],
        reflected: &[attr!("open", "b")],
        members: &[],
    },
    HtmlInterface {
        name: "HTMLMarqueeElement",
        parent: "HTMLElement",
        tags: &["marquee"],
        reflected: &[
            attr!("behavior", "s"),
            attr!("bgColor", "s", attr = "bgcolor"),
            attr!("direction", "s"),
            attr!("height", "s"),
            attr!("scrollAmount", "ul", attr = "scrollamount", missing = "6"),
            attr!("scrollDelay", "ul", attr = "scrolldelay", missing = "85"),
            attr!("trueSpeed", "b", attr = "truespeed"),
            attr!("width", "s"),
        ],
        members: &[],
    },
];

pub(crate) fn bootstrap_script() -> String {
    let mut out = String::from("globalThis.__servalHtmlInterfaceTable = [");
    for (i, interface) in HTML_INTERFACES.iter().enumerate() {
        if i != 0 {
            out.push(',');
        }
        push_interface(&mut out, interface);
    }
    out.push_str("];\n");
    out
}

fn push_interface(out: &mut String, interface: &HtmlInterface) {
    out.push_str("{name:");
    push_js_string(out, interface.name);
    out.push_str(",parent:");
    push_js_string(out, interface.parent);
    out.push_str(",tags:");
    push_string_array(out, interface.tags);
    out.push_str(",reflected:[");
    for (i, attr) in interface.reflected.iter().enumerate() {
        if i != 0 {
            out.push(',');
        }
        push_reflected_attribute(out, attr);
    }
    out.push_str("],members:");
    push_string_array(out, interface.members);
    out.push('}');
}

fn push_reflected_attribute(out: &mut String, attr: &ReflectedAttribute) {
    out.push_str("{idl:");
    push_js_string(out, attr.idl);
    out.push_str(",kind:");
    push_js_string(out, attr.kind);
    out.push_str(",attr:");
    push_optional_js_string(out, attr.attr);
    out.push_str(",keywords:");
    push_string_array(out, attr.keywords);
    out.push_str(",missing:");
    push_optional_js_string(out, attr.missing);
    out.push('}');
}

fn push_string_array(out: &mut String, values: &[&str]) {
    out.push('[');
    for (i, value) in values.iter().enumerate() {
        if i != 0 {
            out.push(',');
        }
        push_js_string(out, value);
    }
    out.push(']');
}

fn push_optional_js_string(out: &mut String, value: Option<&str>) {
    match value {
        Some(value) => push_js_string(out, value),
        None => out.push_str("null"),
    }
}

fn push_js_string(out: &mut String, value: &str) {
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}
