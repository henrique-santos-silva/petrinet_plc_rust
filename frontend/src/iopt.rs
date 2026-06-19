//! XML (PNML/IOPT) parsing and validation.
//!
//! Direct port of `iopt_config_dom_generation.js` and
//! `iopt_config_validation.js`. Produces the same IOPT dictionary the original
//! frontend sent over socket.io, and runs the same validation rules.

use serde_json::{json, Map, Value};
use wasm_bindgen::JsCast;
use web_sys::{DomParser, Element, SupportedType};

// ---- small JS-like number parsers -----------------------------------------

fn parse_int(s: &str) -> i64 {
    let s = s.trim();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if c.is_ascii_digit() || (i == 0 && (c == '-' || c == '+')) {
            out.push(c);
        } else {
            break;
        }
    }
    out.parse().unwrap_or(0)
}

fn parse_float(s: &str) -> f64 {
    let s = s.trim();
    let mut out = String::new();
    let mut seen_dot = false;
    for (i, c) in s.chars().enumerate() {
        if c.is_ascii_digit() || (i == 0 && (c == '-' || c == '+')) {
            out.push(c);
        } else if c == '.' && !seen_dot {
            seen_dot = true;
            out.push(c);
        } else {
            break;
        }
    }
    out.parse().unwrap_or(0.0)
}

// ---- DOM helpers ------------------------------------------------------------

fn query(el: &Element, selector: &str) -> Option<Element> {
    el.query_selector(selector).ok().flatten()
}

fn query_text(el: &Element, selector: &str) -> String {
    query(el, selector)
        .and_then(|e| e.text_content())
        .unwrap_or_default()
}

fn nth_after_comma(text: &str) -> String {
    text.split(',').nth(1).unwrap_or("").to_string()
}

// ---- name sanitisation (port of sanitizePlaceOrTransitionName) -------------

struct Sanitized {
    name: String,
    remaining: Option<String>,
}

fn sanitize_token(s: &str) -> String {
    s.trim().replace(' ', "_").to_lowercase()
}

fn sanitize_place_or_transition_name(id: &str) -> Sanitized {
    match id.find('(') {
        None => Sanitized {
            name: sanitize_token(id),
            remaining: None,
        },
        Some(idx) => {
            let remaining = id[idx..].trim().to_string();
            // strip the surrounding parentheses
            let inner = if remaining.len() >= 2 {
                remaining[1..remaining.len() - 1].to_string()
            } else {
                String::new()
            };
            Sanitized {
                name: sanitize_token(&id[..idx]),
                remaining: Some(inner),
            }
        }
    }
}

fn output_signals_from_place_id(place_id: &str) -> (String, Vec<String>) {
    let s = sanitize_place_or_transition_name(place_id);
    let signals = match s.remaining {
        Some(remaining) => remaining
            .split(';')
            .filter(|x| !x.is_empty())
            .map(|x| x.trim().to_string())
            .collect(),
        None => Vec::new(),
    };
    (s.name, signals)
}

fn transition_expression_from_id(transition_id: &str) -> (String, String) {
    let s = sanitize_place_or_transition_name(transition_id);
    (s.name, s.remaining.unwrap_or_else(|| "true".to_string()))
}

// ---- XML -> IOPT (port of petrinet_xml2json) -------------------------------

pub fn xml_to_iopt(xml: &str) -> Result<Value, String> {
    let parser = DomParser::new().map_err(|_| "DOMParser unavailable".to_string())?;
    let doc = parser
        .parse_from_string(xml, SupportedType::TextXml)
        .map_err(|_| "invalid XML".to_string())?;

    let mut places = Vec::new();
    let mut instantaneous = Vec::new();
    let mut timed = Vec::new();
    let mut arcs = Vec::new();

    // marking_to_output_expressions accumulator
    let mut output_to_places: Vec<(String, Vec<String>)> =
        (0..=15).map(|i| (format!("DO{i}"), Vec::new())).collect();

    // places
    let place_nodes = doc
        .query_selector_all("place")
        .map_err(|_| "querySelectorAll failed".to_string())?;
    for i in 0..place_nodes.length() {
        let el: Element = place_nodes.item(i).unwrap().dyn_into().unwrap();
        let id = el.get_attribute("id").unwrap_or_default();
        let initial_marking = parse_int(&nth_after_comma(&query_text(&el, "initialMarking value")));
        let capacity = parse_int(&query_text(&el, "capacity value"));
        let (x, y) = position_of(&el);

        let (place_name, output_signals) = output_signals_from_place_id(&id);
        places.push(json!({
            "id": place_name,
            "initial_marking": initial_marking,
            "capacity": capacity,
            "graphics": {"x_position": x, "y_position": y}
        }));
        for o in output_signals {
            if let Some(entry) = output_to_places.iter_mut().find(|(k, _)| *k == o) {
                entry.1.push(place_name.clone());
            } else {
                output_to_places.push((o, vec![place_name.clone()]));
            }
        }
    }

    let mut marking_to_output_expressions = Map::new();
    for (output, list) in output_to_places {
        let expr = if list.is_empty() {
            "false".to_string()
        } else {
            list.join(" || ")
        };
        marking_to_output_expressions.insert(output, Value::String(expr));
    }

    // transitions
    let transition_nodes = doc
        .query_selector_all("transition")
        .map_err(|_| "querySelectorAll failed".to_string())?;
    for i in 0..transition_nodes.length() {
        let el: Element = transition_nodes.item(i).unwrap().dyn_into().unwrap();
        let id = el.get_attribute("id").unwrap_or_default();
        let rate = parse_float(&query_text(&el, "rate value"));
        let priority = parse_int(&query_text(&el, "priority value"));
        let is_timed = query_text(&el, "timed value").trim() == "true";
        let (x, y) = position_of(&el);
        let rotation = parse_int(&query_text(&el, "orientation value"));

        let (transition_name, enabling) = transition_expression_from_id(&id);
        let mut obj = json!({
            "id": transition_name,
            "signal_enabling_expression": enabling,
            "rate": rate,
            "priority": priority,
            "graphics": {"x_position": x, "y_position": y, "rotation": rotation}
        });
        if is_timed {
            obj["timer_sec"] = json!(rate);
            timed.push(obj);
        } else {
            instantaneous.push(obj);
        }
    }

    // arcs
    let arc_nodes = doc
        .query_selector_all("arc")
        .map_err(|_| "querySelectorAll failed".to_string())?;
    for i in 0..arc_nodes.length() {
        let el: Element = arc_nodes.item(i).unwrap().dyn_into().unwrap();
        let id = el.get_attribute("id").unwrap_or_default();
        let source =
            sanitize_place_or_transition_name(&el.get_attribute("source").unwrap_or_default()).name;
        let target =
            sanitize_place_or_transition_name(&el.get_attribute("target").unwrap_or_default()).name;
        let weight = parse_int(&nth_after_comma(&query_text(&el, "inscription value")));
        let arc_type = query(&el, "type")
            .and_then(|t| t.get_attribute("value"))
            .unwrap_or_default();

        let mut graphic_path = Vec::new();
        if let Ok(path_nodes) = el.query_selector_all("arcpath") {
            for j in 0..path_nodes.length() {
                let p: Element = path_nodes.item(j).unwrap().dyn_into().unwrap();
                let px = parse_float(&p.get_attribute("x").unwrap_or_default());
                let py = parse_float(&p.get_attribute("y").unwrap_or_default());
                graphic_path.push(json!({"x_position": px, "y_position": py}));
            }
        }

        arcs.push(json!({
            "id": id,
            "source": source,
            "target": target,
            "weight": weight,
            "type": arc_type,
            "graphic_path": graphic_path
        }));
    }

    Ok(json!({
        "places": places,
        "instantaneous_transitions": instantaneous,
        "timed_transitions": timed,
        "arcs": arcs,
        "marking_to_output_expressions": marking_to_output_expressions
    }))
}

fn position_of(el: &Element) -> (f64, f64) {
    let pos = query(el, "graphics position");
    match pos {
        Some(p) => (
            parse_float(&p.get_attribute("x").unwrap_or_default()),
            parse_float(&p.get_attribute("y").unwrap_or_default()),
        ),
        None => (0.0, 0.0),
    }
}

// ---- boolean expression validation (port of is_valid_expression) ----------

fn basic_token_regression(token: &str) -> Option<&'static str> {
    match token {
        "true" => Some("true"),
        "false" => Some("false"),
        "(" => Some("("),
        ")" => Some(")"),
        "||" => Some("|"),
        "&&" => Some("&"),
        "^" => Some("^"),
        "!" => Some("!"),
        "~" => Some("!"),
        "and" | "AND" => Some("&"),
        "or" | "OR" => Some("|"),
        "not" | "NOT" => Some("!"),
        "xor" | "XOR" => Some("^"),
        _ => None,
    }
}

const BASIC_TOKENS: &[&str] = &[
    "(", ")", "||", "&&", "^", "!", "~", "true", "false", "and", "or", "not", "xor",
];

/// Returns `(is_valid, normalised_expression)`.
pub fn is_valid_expression(expression: &str, extra_tokens: &[String]) -> (bool, String) {
    let expression: String = expression.chars().filter(|c| *c != ' ').collect();
    if expression.is_empty() {
        return (false, String::new());
    }

    let extra_lower: Vec<String> = extra_tokens.iter().map(|t| t.to_lowercase()).collect();

    let chars: Vec<char> = expression.chars().collect();
    let mut tokens: Vec<String> = Vec::new();
    let mut tokens_bool_substitution: Vec<String> = Vec::new();
    let mut token_tmp: Option<String> = None;

    let mut p0 = 0usize;
    let mut p1 = 0usize;
    while p0 <= p1 && p1 <= chars.len() {
        let substring: String = chars[p0..p1].iter().collect();
        let substring_lower = substring.to_lowercase();

        if BASIC_TOKENS.contains(&substring_lower.as_str()) {
            tokens.push(substring.clone());
            tokens_bool_substitution.push(
                basic_token_regression(&substring_lower)
                    .unwrap_or("")
                    .to_string(),
            );
            p0 = p1;
            continue;
        } else if (substring_lower.starts_with("di") || substring_lower.starts_with('p'))
            && extra_lower.contains(&substring_lower)
        {
            token_tmp = Some(substring.clone());
        } else if substring_lower.starts_with("di")
            || (substring_lower.starts_with('p') && !extra_lower.contains(&substring_lower))
        {
            if let Some(t) = token_tmp.take() {
                tokens.push(t);
                tokens_bool_substitution.push("true".to_string());
                p1 -= 1;
                p0 = p1;
                continue;
            }
        }

        p1 += 1;
    }

    if let Some(t) = token_tmp.take() {
        tokens.push(t);
        tokens_bool_substitution.push("true".to_string());
    }

    if tokens.concat() != expression {
        return (false, String::new());
    }

    let final_expression = tokens
        .iter()
        .map(|t| {
            basic_token_regression(t)
                .map(|s| s.to_string())
                .unwrap_or_else(|| t.clone())
        })
        .collect::<Vec<_>>()
        .join(" ")
        + " ";

    let proxy = tokens_bool_substitution.join(" ");
    // Validate using the JS engine, exactly like the original.
    match js_sys::eval(&proxy) {
        Ok(_) => (true, final_expression),
        Err(_) => (false, String::new()),
    }
}

fn is_valid_python_variable_name(name: &str) -> bool {
    const KEYWORDS: &[&str] = &[
        "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class",
        "continue", "def", "del", "elif", "else", "except", "finally", "for", "from", "global",
        "if", "import", "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise", "return",
        "try", "while", "with", "yield",
    ];
    if KEYWORDS.contains(&name) {
        return false;
    }
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Port of `validadeIOPT`. Mutates `iopt` (normalising expressions) and returns
/// the error map (empty == valid).
pub fn validate_iopt(iopt: &mut Value) -> Map<String, Value> {
    let mut errors = Map::new();

    let place_ids: Vec<String> = iopt
        .get("places")
        .and_then(|p| p.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|p| p.get("id").and_then(|v| v.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default();

    // --- expected DO0..DO15 ---
    let expected: Vec<String> = (0..=15).map(|i| format!("DO{i}")).collect();
    let actual: Vec<String> = iopt
        .get("marking_to_output_expressions")
        .and_then(|m| m.as_object())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();

    let missing: Vec<String> = expected
        .iter()
        .filter(|e| !actual.contains(e))
        .cloned()
        .collect();
    let exceeding: Vec<String> = actual
        .iter()
        .filter(|a| !expected.contains(a))
        .cloned()
        .collect();
    if !missing.is_empty() {
        errors.insert("missingDigitalOutputs".into(), json!(missing));
    }
    if !exceeding.is_empty() {
        errors.insert("exceedingDigitalOutputs".into(), json!(exceeding));
    }

    // --- output activation expressions ---
    let mut output_errors = Map::new();
    if let Some(map) = iopt
        .get_mut("marking_to_output_expressions")
        .and_then(|m| m.as_object_mut())
    {
        let keys: Vec<String> = map.keys().cloned().collect();
        for key in keys {
            let expr = map
                .get(&key)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let (valid, new_string) = is_valid_expression(&expr, &place_ids);
            if !valid {
                output_errors.insert(key, json!(expr));
            } else {
                map.insert(key, Value::String(new_string));
            }
        }
    }
    if !output_errors.is_empty() {
        errors.insert(
            "outputActivationExpressionErrors".into(),
            Value::Object(output_errors),
        );
    }

    // --- transition signal-enabling expressions ---
    let di_tokens: Vec<String> = (0..8).map(|i| format!("DI{i}")).collect();
    let mut transition_errors = Map::new();
    for group in ["instantaneous_transitions", "timed_transitions"] {
        if let Some(arr) = iopt.get_mut(group).and_then(|v| v.as_array_mut()) {
            for t in arr.iter_mut() {
                let name = t
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let expr = t
                    .get("signal_enabling_expression")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let (valid, new_string) = is_valid_expression(&expr, &di_tokens);
                if !valid {
                    transition_errors.insert(name, json!(expr));
                } else {
                    t["signal_enabling_expression"] = Value::String(new_string);
                }
            }
        }
    }
    if !transition_errors.is_empty() {
        errors.insert(
            "transitionSignalEnablingExpresionErrors".into(),
            Value::Object(transition_errors),
        );
    }

    // --- place names ---
    let mut invalid_place_names = Vec::new();
    for pname in &place_ids {
        let starts_with_p = pname
            .chars()
            .next()
            .map(|c| c.eq_ignore_ascii_case(&'p'))
            .unwrap_or(false);
        if !is_valid_python_variable_name(pname) || !starts_with_p {
            invalid_place_names.push(pname.clone());
        }
    }
    if !invalid_place_names.is_empty() {
        errors.insert("invalidPlaceNameErrors".into(), json!(invalid_place_names));
    }

    // --- transition names ---
    let mut invalid_transition_names = Vec::new();
    for group in ["instantaneous_transitions", "timed_transitions"] {
        if let Some(arr) = iopt.get(group).and_then(|v| v.as_array()) {
            for t in arr {
                if let Some(name) = t.get("id").and_then(|v| v.as_str()) {
                    if !is_valid_python_variable_name(name) {
                        invalid_transition_names.push(name.to_string());
                    }
                }
            }
        }
    }
    if !invalid_transition_names.is_empty() {
        errors.insert(
            "invalidTransitionNameErrors".into(),
            json!(invalid_transition_names),
        );
    }

    errors
}
