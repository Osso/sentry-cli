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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_transaction_spans_from_array_and_object_shapes() {
        let transactions = serde_json::json!([transaction(
            "root",
            None,
            "http.server",
            "/home",
            0.0,
            0.250,
            vec![span(
                "child",
                Some("root"),
                "db",
                "SELECT",
                0.010,
                0.050,
                "ok"
            )]
        )]);
        let wrapped = serde_json::json!({"transactions": transactions});
        let events = HashMap::new();

        let array_spans = extract_spans(&transactions, &events);
        let wrapped_spans = extract_spans(&wrapped, &events);

        assert_eq!(array_spans.len(), 2);
        assert_eq!(wrapped_spans.len(), 2);
        assert_eq!(array_spans[0].span_id, "root");
        assert_eq!(array_spans[0].duration_ms, 250.0);
        assert_eq!(array_spans[1].parent_span_id.as_deref(), Some("root"));
    }

    #[test]
    fn event_spans_override_embedded_transaction_spans() {
        let data = serde_json::json!([transaction(
            "root",
            None,
            "http.server",
            "/home",
            0.0,
            0.250,
            vec![span(
                "embedded",
                Some("root"),
                "db",
                "embedded",
                0.010,
                0.020,
                "ok"
            )]
        )]);
        let mut events = HashMap::new();
        events.insert(
            "root".to_string(),
            serde_json::json!({
                "entries": [
                    {"type": "spans", "data": [{"span_id": "event", "parent_span_id": "root", "op": "cache", "description": "event", "timestamp": 0.040, "start_timestamp": 0.010, "status": "deadline_exceeded"}]}
                ]
            }),
        );

        let spans = extract_spans(&data, &events);

        assert_eq!(spans.len(), 2);
        assert_eq!(spans[1].span_id, "event");
        assert_eq!(spans[1].duration_ms, 30.0);
        assert_eq!(status_marker(&spans[1].status), " [deadline_exceeded]");
    }

    #[test]
    fn transaction_event_refs_skip_incomplete_rows() {
        let refs = extract_transaction_event_refs(&serde_json::json!({
            "transactions": [
                {"contexts": {"trace": {"span_id": "a"}}, "projectSlug": "web", "eventID": "event-a"},
                {"contexts": {"trace": {"span_id": ""}}, "projectSlug": "web", "eventID": "skip"},
                {"contexts": {"trace": {"span_id": "b"}}, "projectSlug": "", "eventID": "skip"}
            ]
        }));

        assert_eq!(
            refs,
            vec![("a".to_string(), "web".to_string(), "event-a".to_string())]
        );
    }

    #[test]
    fn root_detection_handles_missing_parents() {
        let spans = vec![
            Span {
                span_id: "root".to_string(),
                parent_span_id: None,
                op: "root".to_string(),
                description: String::new(),
                duration_ms: 50.0,
                status: String::new(),
            },
            Span {
                span_id: "orphan".to_string(),
                parent_span_id: Some("missing".to_string()),
                op: "orphan".to_string(),
                description: String::new(),
                duration_ms: 20.0,
                status: String::new(),
            },
        ];

        assert_eq!(root_ids(&spans), vec!["root", "orphan"]);
        assert_eq!(slow_marker(150.0), " ⚠ SLOW");
        assert_eq!(slow_marker(50.0), "");
        assert_eq!(status_marker("ok"), "");
        assert_eq!(status_marker("internal_error"), " [internal_error]");
    }

    #[test]
    fn print_functions_accept_empty_and_populated_data() {
        let events = HashMap::new();

        print_trace(&serde_json::json!({}), &events);
        print_trace(
            &serde_json::json!([transaction("root", None, "http", "/", 0.0, 0.010, vec![])]),
            &events,
        );
        print_event_spans(&serde_json::json!({
            "entries": [
                {"type": "spans", "data": [span("child", Some("root"), "db", "SELECT", 0.0, 0.010, "ok")]}
            ]
        }));
    }

    fn transaction(
        span_id: &str,
        parent_span_id: Option<&str>,
        op: &str,
        transaction_name: &str,
        start: f64,
        end: f64,
        spans: Vec<Value>,
    ) -> Value {
        serde_json::json!({
            "contexts": {"trace": {"span_id": span_id, "parent_span_id": parent_span_id, "op": op, "status": "ok"}},
            "transaction": transaction_name,
            "start_timestamp": start,
            "timestamp": end,
            "spans": spans
        })
    }

    fn span(
        span_id: &str,
        parent_span_id: Option<&str>,
        op: &str,
        description: &str,
        start: f64,
        end: f64,
        status: &str,
    ) -> Value {
        serde_json::json!({
            "span_id": span_id,
            "parent_span_id": parent_span_id,
            "op": op,
            "description": description,
            "start_timestamp": start,
            "timestamp": end,
            "status": status
        })
    }
}
