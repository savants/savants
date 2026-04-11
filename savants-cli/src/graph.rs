use redis::{Client, Commands, RedisResult, Value};
use std::env;

pub struct GraphClient {
    client: Client,
    graph_name: String,
}

#[derive(Debug, Clone)]
pub struct QueryResult {
    pub rows: Vec<Vec<GraphValue>>,
}

#[derive(Debug, Clone)]
pub enum GraphValue {
    String(String),
    Integer(i64),
    Float(f64),
    Boolean(bool),
    Null,
    Array(Vec<GraphValue>),
}

impl GraphValue {
    pub fn as_str(&self) -> &str {
        match self {
            GraphValue::String(s) => s,
            _ => "",
        }
    }

    pub fn as_i64(&self) -> i64 {
        match self {
            GraphValue::Integer(i) => *i,
            GraphValue::Float(f) => *f as i64,
            GraphValue::String(s) => s.parse().unwrap_or(0),
            _ => 0,
        }
    }

    pub fn as_f64(&self) -> f64 {
        match self {
            GraphValue::Float(f) => *f,
            GraphValue::Integer(i) => *i as f64,
            GraphValue::String(s) => s.parse().unwrap_or(0.0),
            _ => 0.0,
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, GraphValue::Null)
    }
}

impl GraphClient {
    pub fn new(graph_name: &str) -> RedisResult<Self> {
        let host = env::var("FALKORDB_HOST").unwrap_or_else(|_| "localhost".to_string());
        let port = env::var("FALKORDB_PORT").unwrap_or_else(|_| "16379".to_string());
        let url = format!("redis://{}:{}", host, port);
        let client = Client::open(url)?;
        Ok(Self {
            client,
            graph_name: graph_name.to_string(),
        })
    }

    pub fn query(&self, cypher: &str, params: &[(&str, &str)]) -> RedisResult<QueryResult> {
        let mut conn = self.client.get_connection()?;

        // Build parameterized cypher string
        let param_str = if params.is_empty() {
            String::new()
        } else {
            let parts: Vec<String> = params
                .iter()
                .map(|(k, v)| format!("{}='{}'", k, v.replace('\'', "\\'")))
                .collect();
            format!("CYPHER {} ", parts.join(" "))
        };

        let full_query = format!("{}{}", param_str, cypher);

        let result: Value = redis::cmd("GRAPH.QUERY")
            .arg(&self.graph_name)
            .arg(&full_query)
            .arg("--compact")
            .query(&mut conn)?;

        Ok(parse_graph_result(result))
    }

    pub fn is_connected(&self) -> bool {
        self.client.get_connection().is_ok()
    }
}

fn parse_graph_result(value: Value) -> QueryResult {
    let rows = match value {
        Value::Bulk(ref parts) if parts.len() >= 2 => {
            if let Value::Bulk(ref result_rows) = parts[1] {
                result_rows
                    .iter()
                    .map(|row| {
                        if let Value::Bulk(ref cols) = row {
                            cols.iter().map(parse_value).collect()
                        } else {
                            vec![parse_value(row)]
                        }
                    })
                    .collect()
            } else {
                vec![]
            }
        }
        _ => vec![],
    };

    QueryResult { rows }
}

fn parse_value(v: &Value) -> GraphValue {
    match v {
        Value::Data(bytes) => {
            GraphValue::String(String::from_utf8_lossy(bytes).to_string())
        }
        Value::Status(s) => GraphValue::String(s.clone()),
        Value::Int(i) => GraphValue::Integer(*i),
        Value::Okay => GraphValue::String("OK".to_string()),
        Value::Nil => GraphValue::Null,
        Value::Bulk(arr) => {
            // FalkorDB compact format: [type_id, value]
            if arr.len() == 2 {
                if let Value::Int(type_id) = &arr[0] {
                    return match type_id {
                        1 => GraphValue::Null,
                        2 => parse_value(&arr[1]),
                        3 => parse_value(&arr[1]),
                        4 => {
                            if let Value::Status(s) = &arr[1] {
                                GraphValue::Boolean(s == "true")
                            } else {
                                parse_value(&arr[1])
                            }
                        }
                        5 => {
                            // Double encoded as string in compact mode
                            if let Value::Data(bytes) = &arr[1] {
                                let s = String::from_utf8_lossy(bytes);
                                GraphValue::Float(s.parse().unwrap_or(0.0))
                            } else {
                                parse_value(&arr[1])
                            }
                        }
                        6 => {
                            if let Value::Bulk(inner) = &arr[1] {
                                GraphValue::Array(inner.iter().map(parse_value).collect())
                            } else {
                                parse_value(&arr[1])
                            }
                        }
                        _ => parse_value(&arr[1]),
                    };
                }
            }
            GraphValue::Array(arr.iter().map(parse_value).collect())
        }
    }
}
