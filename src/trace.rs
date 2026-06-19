use serde_json::Value;
use std::collections::HashMap;

pub const SLOW_SPAN_MS: f64 = 100.0;

struct Span {
    span_id: String,
    parent_span_id: Option<String>,
    op: String,
    description: String,
    duration_ms: f64,
    status: String,
}

fn extract_spans(data: &Value, events: &HashMap<String, Value>) -> Vec<Span> {
    let mut spans = Vec::new();

    // The trace response has a list of transactions, each containing spans
    if let Some(transactions) = data.as_array() {
        for txn in transactions {
            collect_transaction_spans(txn, events, &mut spans);
        }
    } else if let Some(transactions) = data["transactions"].as_array() {
        for txn in transactions {
            collect_transaction_spans(txn, events, &mut spans);
        }
    }

    spans
}

fn parse_transaction_span(txn: &Value) -> Option<Span> {
    let trace = &txn["contexts"]["trace"];
    let span_id = trace["span_id"].as_str()?.to_string();

    Some(Span {
        span_id,
        parent_span_id: trace["parent_span_id"].as_str().map(|s| s.to_string()),
        op: trace["op"].as_str().unwrap_or("transaction").to_string(),
        description: txn["transaction"].as_str().unwrap_or("").to_string(),
        duration_ms: compute_duration_ms(&txn["start_timestamp"], &txn["timestamp"]),
        status: trace["status"].as_str().unwrap_or("").to_string(),
    })
}

fn collect_transaction_spans(txn: &Value, events: &HashMap<String, Value>, spans: &mut Vec<Span>) {
    let txn_span = match parse_transaction_span(txn) {
        Some(span) => span,
        None => return,
    };
    let txn_span_id = txn_span.span_id.clone();
    spans.push(txn_span);

    if let Some(event) = events.get(&txn_span_id) {
        spans.extend(extract_event_spans(event));
    } else if let Some(span_list) = txn["spans"].as_array() {
        spans.extend(span_list.iter().map(parse_span));
    }
}

fn parse_span(s: &Value) -> Span {
    Span {
        span_id: s["span_id"].as_str().unwrap_or("").to_string(),
        parent_span_id: s["parent_span_id"].as_str().map(|v| v.to_string()),
        op: s["op"].as_str().unwrap_or("").to_string(),
        description: s["description"].as_str().unwrap_or("").to_string(),
        duration_ms: compute_duration_ms(&s["start_timestamp"], &s["timestamp"]),
        status: s["status"].as_str().unwrap_or("").to_string(),
    }
}

fn compute_duration_ms(start: &Value, end: &Value) -> f64 {
    let s = start.as_f64().unwrap_or(0.0);
    let e = end.as_f64().unwrap_or(0.0);
    (e - s) * 1000.0
}

fn slow_marker(duration_ms: f64) -> &'static str {
    if duration_ms > SLOW_SPAN_MS {
        " ⚠ SLOW"
    } else {
        ""
    }
}

fn status_marker(status: &str) -> String {
    if !status.is_empty() && status != "ok" {
        format!(" [{}]", status)
    } else {
        String::new()
    }
}

fn print_span_tree(spans: &[Span], parent_id: Option<&str>, prefix: &str, is_last: bool) {
    let children: Vec<&Span> = spans
        .iter()
        .filter(|s| s.parent_span_id.as_deref() == parent_id)
        .collect();

    for (i, span) in children.iter().enumerate() {
        let last = i == children.len() - 1;
        let connector = if last { "└── " } else { "├── " };
        let child_prefix = if parent_id.is_none() {
            String::new()
        } else if is_last {
            format!("{}    ", prefix)
        } else {
            format!("{}│   ", prefix)
        };

        let slow_marker = slow_marker(span.duration_ms);
        let status_str = status_marker(&span.status);

        println!(
            "{}{}{} — {} ({:.1}ms){}{}",
            prefix, connector, span.op, span.description, span.duration_ms, status_str, slow_marker
        );

        print_span_tree(spans, Some(&span.span_id), &child_prefix, last);
    }
}

/// Extract spans from an event's `entries` array (type: "spans")
fn extract_event_spans(event: &Value) -> Vec<Span> {
    let Some(entries) = event["entries"].as_array() else {
        return Vec::new();
    };

    entries
        .iter()
        .filter(|entry| entry["type"].as_str() == Some("spans"))
        .flat_map(|entry| entry["data"].as_array().into_iter().flatten())
        .map(parse_span)
        .collect()
}

pub fn print_event_spans(event: &Value) {
    // Print transaction root span from the event itself
    let root_op = event["contexts"]["trace"]["op"]
        .as_str()
        .unwrap_or("transaction");
    let root_desc = event["transaction"]
        .as_str()
        .unwrap_or(event["message"].as_str().unwrap_or(""));
    let root_duration = compute_duration_ms(&event["startTimestamp"], &event["timestamp"]);
    let slow_marker = slow_marker(root_duration);

    println!(
        "{} — {} ({:.1}ms){}",
        root_op, root_desc, root_duration, slow_marker
    );

    let spans = extract_event_spans(event);
    if spans.is_empty() {
        println!("  (no child spans)");
        return;
    }

    let root_span_id = event["contexts"]["trace"]["span_id"].as_str().unwrap_or("");
    print_span_tree(&spans, Some(root_span_id), "", true);
}

/// Extract transaction info needed to fetch events: returns Vec of (span_id, project_slug, event_id)
pub fn extract_transaction_event_refs(data: &Value) -> Vec<(String, String, String)> {
    let mut refs = Vec::new();
    let transactions = if let Some(arr) = data.as_array() {
        arr
    } else if let Some(arr) = data["transactions"].as_array() {
        arr
    } else {
        return refs;
    };

    for txn in transactions {
        let span_id = txn["contexts"]["trace"]["span_id"].as_str().unwrap_or("");
        let project_slug = txn["projectSlug"].as_str().unwrap_or("");
        let event_id = txn["eventID"].as_str().unwrap_or("");
        if !span_id.is_empty() && !project_slug.is_empty() && !event_id.is_empty() {
            refs.push((
                span_id.to_string(),
                project_slug.to_string(),
                event_id.to_string(),
            ));
        }
    }

    refs
}

fn is_root_span(span: &Span, spans: &[Span]) -> bool {
    span.parent_span_id.is_none()
        || !spans
            .iter()
            .any(|parent| Some(&parent.span_id) == span.parent_span_id.as_ref())
}

fn root_ids(spans: &[Span]) -> Vec<&str> {
    spans
        .iter()
        .filter(|span| is_root_span(span, spans))
        .map(|span| span.span_id.as_str())
        .collect()
}

fn print_root_span(root: &Span) {
    let slow_marker = slow_marker(root.duration_ms);
    let status_str = status_marker(&root.status);
    println!(
        "{} — {} ({:.1}ms){}{}",
        root.op, root.description, root.duration_ms, status_str, slow_marker
    );
}

pub fn print_trace(data: &Value, events: &HashMap<String, Value>) {
    let spans = extract_spans(data, events);

    if spans.is_empty() {
        println!("No spans found in trace.");
        println!("{}", serde_json::to_string_pretty(data).unwrap_or_default());
        return;
    }

    let root_ids = root_ids(&spans);

    println!("Trace ({} spans):", spans.len());
    for (i, root_id) in root_ids.iter().enumerate() {
        let root = spans.iter().find(|s| s.span_id == *root_id).unwrap();
        let last = i == root_ids.len() - 1;
        print_root_span(root);
        print_span_tree(&spans, Some(root_id), "", last);
    }
}
