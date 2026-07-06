//! Graph algorithms on the code index using petgraph.
//!
//! Loads the same ParseResult JSON that code_parser produces,
//! builds a petgraph::DiGraph, and runs algorithms:
//! - cycles (Tarjan SCC)
//! - page_rank (iterative)
//! - shortest_path (BFS)
//! - bridge_nodes (betweenness centrality)
//! - communities (weakly connected components)

use crate::code_parser::ParseResult;
use colored::*;
use petgraph::algo::tarjan_scc;
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::EdgeRef;
use std::collections::{HashMap, HashSet, VecDeque};

/// Node data stored in the graph.
#[derive(Debug, Clone)]
pub struct FnNode {
    pub name: String,
    pub file: String,
    pub line: usize,
}

/// Built graph with lookup maps.
pub struct CodeGraph {
    pub graph: DiGraph<FnNode, ()>,
    /// name -> node index (if unique) or first occurrence
    name_to_idx: HashMap<String, Vec<NodeIndex>>,
}

impl CodeGraph {
    /// Build a directed graph from a ParseResult.
    pub fn from_parse_result(pr: &ParseResult) -> Self {
        let mut graph = DiGraph::new();
        let mut name_to_idx: HashMap<String, Vec<NodeIndex>> = HashMap::new();
        // key = (name, file) to dedup
        let mut key_to_idx: HashMap<(String, String), NodeIndex> = HashMap::new();

        // Add nodes for every function entity
        for entity in &pr.entities {
            if entity.kind != "function" {
                continue;
            }
            let key = (entity.name.clone(), entity.file.clone());
            if key_to_idx.contains_key(&key) {
                continue;
            }
            let idx = graph.add_node(FnNode {
                name: entity.name.clone(),
                file: entity.file.clone(),
                line: entity.line,
            });
            key_to_idx.insert(key, idx);
            name_to_idx
                .entry(entity.name.clone())
                .or_default()
                .push(idx);
        }

        // Add edges from call_sites
        for cs in &pr.call_sites {
            // Resolve caller: match by name + file
            let caller_idxs: Vec<NodeIndex> = key_to_idx
                .iter()
                .filter(|((n, f), _)| n == &cs.caller_name && f == &cs.caller_file)
                .map(|(_, &idx)| idx)
                .collect();
            let caller_idx = if let Some(&idx) = caller_idxs.first() {
                idx
            } else if let Some(idxs) = name_to_idx.get(&cs.caller_name) {
                idxs[0]
            } else {
                // Caller not in entities, create a node for it
                let idx = graph.add_node(FnNode {
                    name: cs.caller_name.clone(),
                    file: cs.caller_file.clone(),
                    line: 0,
                });
                let key = (cs.caller_name.clone(), cs.caller_file.clone());
                key_to_idx.insert(key, idx);
                name_to_idx
                    .entry(cs.caller_name.clone())
                    .or_default()
                    .push(idx);
                idx
            };

            // Resolve callee: match by name (could be in any file)
            let callee_idx = if let Some(idxs) = name_to_idx.get(&cs.callee_name) {
                idxs[0]
            } else {
                // Callee not in entities, create a stub node
                let idx = graph.add_node(FnNode {
                    name: cs.callee_name.clone(),
                    file: String::new(),
                    line: 0,
                });
                name_to_idx
                    .entry(cs.callee_name.clone())
                    .or_default()
                    .push(idx);
                idx
            };

            // Avoid self-loops and duplicate edges
            if caller_idx != callee_idx {
                // Check if edge already exists
                if !graph.edges(caller_idx).any(|e| e.target() == callee_idx) {
                    graph.add_edge(caller_idx, callee_idx, ());
                }
            }
        }

        CodeGraph {
            graph,
            name_to_idx,
        }
    }

    /// Find circular dependencies using Tarjan's SCC algorithm.
    pub fn find_cycles(&self) -> String {
        let sccs = tarjan_scc(&self.graph);
        // Filter to SCCs with more than 1 node (actual cycles)
        let cycles: Vec<_> = sccs
            .into_iter()
            .filter(|scc| scc.len() > 1)
            .collect();

        if cycles.is_empty() {
            return format!(
                "{}\nNo circular dependencies found.\n",
                "=== Circular Dependencies ===".bold()
            );
        }

        let mut out = format!(
            "{}\n{} cycle(s) found:\n",
            "=== Circular Dependencies ===".bold(),
            cycles.len()
        );

        for (i, scc) in cycles.iter().enumerate() {
            out.push_str(&format!(
                "\n{}Cycle {} ({} functions):{}\n",
                "".clear(),
                i + 1,
                scc.len(),
                "".clear()
            ));

            // Build the cycle path
            let names: Vec<_> = scc
                .iter()
                .map(|&idx| self.graph[idx].name.clone())
                .collect();
            let path = format!(
                "  {} -> {}",
                names.join(" -> "),
                names.first().unwrap_or(&String::new())
            );
            out.push_str(&format!("{}\n", path.cyan()));

            // Collect files
            let files: HashSet<_> = scc
                .iter()
                .map(|&idx| self.graph[idx].file.clone())
                .filter(|f| !f.is_empty())
                .collect();
            if !files.is_empty() {
                let mut sorted_files: Vec<_> = files.into_iter().collect();
                sorted_files.sort();
                out.push_str(&format!("  Files: {}\n", sorted_files.join(", ")));
            }
        }

        out
    }

    /// Compute PageRank to find the most important functions.
    pub fn page_rank(&self, top_n: usize) -> String {
        let node_count = self.graph.node_count();
        if node_count == 0 {
            return format!(
                "{}\nNo functions in the graph.\n",
                "=== Code Hotspots (PageRank) ===".bold()
            );
        }

        let damping = 0.85;
        let iterations = 100;
        let initial = 1.0 / node_count as f64;

        let indices: Vec<NodeIndex> = self.graph.node_indices().collect();
        let mut scores: HashMap<NodeIndex, f64> = indices.iter().map(|&idx| (idx, initial)).collect();

        for _ in 0..iterations {
            let mut new_scores: HashMap<NodeIndex, f64> =
                indices.iter().map(|&idx| (idx, (1.0 - damping) / node_count as f64)).collect();

            for &idx in &indices {
                let out_degree = self.graph.edges(idx).count();
                if out_degree == 0 {
                    // Dangling node: distribute evenly
                    let share = scores[&idx] * damping / node_count as f64;
                    for &other in &indices {
                        *new_scores.get_mut(&other).unwrap() += share;
                    }
                } else {
                    let share = scores[&idx] * damping / out_degree as f64;
                    for edge in self.graph.edges(idx) {
                        *new_scores.get_mut(&edge.target()).unwrap() += share;
                    }
                }
            }

            scores = new_scores;
        }

        // Sort by score descending
        let mut ranked: Vec<_> = scores.into_iter().collect();
        ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let mut out = format!(
            "{}\nTop {} most-connected functions:\n\n",
            "=== Code Hotspots (PageRank) ===".bold(),
            top_n
        );

        for (i, (idx, score)) in ranked.iter().take(top_n).enumerate() {
            let node = &self.graph[*idx];
            let callers = self
                .graph
                .neighbors_directed(*idx, petgraph::Direction::Incoming)
                .count();
            let callees = self
                .graph
                .neighbors_directed(*idx, petgraph::Direction::Outgoing)
                .count();

            let location = if !node.file.is_empty() && node.line > 0 {
                format!("{}:{}", node.file, node.line)
            } else if !node.file.is_empty() {
                node.file.clone()
            } else {
                "unknown".to_string()
            };

            out.push_str(&format!(
                "  {:>2}. {:<30} ({})  score={:.4}  callers={}  callees={}\n",
                i + 1,
                node.name,
                location.dimmed(),
                score,
                callers,
                callees
            ));
        }

        out
    }

    /// Find shortest call path between two functions using BFS.
    pub fn shortest_path(&self, from_fn: &str, to_fn: &str) -> String {
        let from_idx = match self.resolve_node(from_fn) {
            Some(idx) => idx,
            None => {
                return format!(
                    "{}: function '{}' not found in the graph.",
                    "Error".red(),
                    from_fn
                );
            }
        };

        let to_idx = match self.resolve_node(to_fn) {
            Some(idx) => idx,
            None => {
                return format!(
                    "{}: function '{}' not found in the graph.",
                    "Error".red(),
                    to_fn
                );
            }
        };

        // BFS from source to target
        let mut visited: HashSet<NodeIndex> = HashSet::new();
        let mut queue: VecDeque<(NodeIndex, Vec<NodeIndex>)> = VecDeque::new();
        queue.push_back((from_idx, vec![from_idx]));
        visited.insert(from_idx);

        while let Some((current, path)) = queue.pop_front() {
            if current == to_idx {
                // Found the path
                let names: Vec<_> = path
                    .iter()
                    .map(|&idx| self.graph[idx].name.clone())
                    .collect();
                let files: HashSet<_> = path
                    .iter()
                    .map(|&idx| self.graph[idx].file.clone())
                    .filter(|f| !f.is_empty())
                    .collect();

                let mut out = format!("{}\n", "=== Shortest Path ===".bold());
                out.push_str(&format!("{}\n", names.join(" -> ").cyan()));
                out.push_str(&format!(
                    "  {} hop(s)",
                    path.len() - 1
                ));
                if !files.is_empty() {
                    let mut sorted: Vec<_> = files.into_iter().collect();
                    sorted.sort();
                    if sorted.len() == 1 {
                        out.push_str(&format!(", all in {}", sorted[0]));
                    } else {
                        out.push_str(&format!(", across: {}", sorted.join(", ")));
                    }
                }
                out.push('\n');
                return out;
            }

            for neighbor in self.graph.neighbors(current) {
                if visited.insert(neighbor) {
                    let mut new_path = path.clone();
                    new_path.push(neighbor);
                    queue.push_back((neighbor, new_path));
                }
            }
        }

        format!(
            "{}\nNo path found from '{}' to '{}'.\n",
            "=== Shortest Path ===".bold(),
            from_fn,
            to_fn
        )
    }

    /// Find bridge/bottleneck nodes using approximate betweenness centrality.
    pub fn bridge_nodes(&self, top_n: usize) -> String {
        let indices: Vec<NodeIndex> = self.graph.node_indices().collect();
        let node_count = indices.len();

        if node_count == 0 {
            return format!(
                "{}\nNo functions in the graph.\n",
                "=== Bridge/Bottleneck Functions ===".bold()
            );
        }

        // Betweenness centrality: for each pair (s,t), find shortest path,
        // count how many times each intermediate node appears.
        // For large graphs, sample a subset of source nodes.
        let max_sources = 200.min(node_count);
        let sources: Vec<NodeIndex> = if node_count <= max_sources {
            indices.clone()
        } else {
            // Sample evenly spaced nodes
            let step = node_count / max_sources;
            indices.iter().step_by(step).copied().take(max_sources).collect()
        };

        let mut centrality: HashMap<NodeIndex, f64> = indices.iter().map(|&idx| (idx, 0.0)).collect();

        for &source in &sources {
            // BFS from source, record predecessors
            let mut dist: HashMap<NodeIndex, usize> = HashMap::new();
            let mut sigma: HashMap<NodeIndex, f64> = HashMap::new(); // number of shortest paths
            let mut pred: HashMap<NodeIndex, Vec<NodeIndex>> = HashMap::new();
            let mut stack: Vec<NodeIndex> = Vec::new();

            dist.insert(source, 0);
            sigma.insert(source, 1.0);
            let mut queue: VecDeque<NodeIndex> = VecDeque::new();
            queue.push_back(source);

            while let Some(v) = queue.pop_front() {
                stack.push(v);
                let d_v = dist[&v];
                for neighbor in self.graph.neighbors(v) {
                    if !dist.contains_key(&neighbor) {
                        dist.insert(neighbor, d_v + 1);
                        queue.push_back(neighbor);
                    }
                    if dist[&neighbor] == d_v + 1 {
                        *sigma.entry(neighbor).or_insert(0.0) += sigma.get(&v).copied().unwrap_or(0.0);
                        pred.entry(neighbor).or_default().push(v);
                    }
                }
            }

            // Accumulate dependency
            let mut delta: HashMap<NodeIndex, f64> = HashMap::new();
            while let Some(w) = stack.pop() {
                if let Some(preds) = pred.get(&w) {
                    let s_w = sigma.get(&w).copied().unwrap_or(1.0);
                    let d_w = delta.get(&w).copied().unwrap_or(0.0);
                    for &v in preds {
                        let s_v = sigma.get(&v).copied().unwrap_or(1.0);
                        let contrib = (s_v / s_w) * (1.0 + d_w);
                        *delta.entry(v).or_insert(0.0) += contrib;
                    }
                }
                if w != source {
                    *centrality.get_mut(&w).unwrap() += delta.get(&w).copied().unwrap_or(0.0);
                }
            }
        }

        // Normalize
        let scale = if node_count > 2 {
            1.0 / ((node_count - 1) as f64 * (node_count - 2) as f64)
        } else {
            1.0
        };
        for val in centrality.values_mut() {
            *val *= scale;
        }

        let mut ranked: Vec<_> = centrality.into_iter().collect();
        ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let mut out = format!(
            "{}\nTop {} bridge/bottleneck functions (betweenness centrality):\n\n",
            "=== Bridge/Bottleneck Functions ===".bold(),
            top_n
        );

        for (i, (idx, score)) in ranked.iter().take(top_n).enumerate() {
            let node = &self.graph[*idx];
            let in_deg = self
                .graph
                .neighbors_directed(*idx, petgraph::Direction::Incoming)
                .count();
            let out_deg = self
                .graph
                .neighbors_directed(*idx, petgraph::Direction::Outgoing)
                .count();

            let location = if !node.file.is_empty() && node.line > 0 {
                format!("{}:{}", node.file, node.line)
            } else if !node.file.is_empty() {
                node.file.clone()
            } else {
                "unknown".to_string()
            };

            out.push_str(&format!(
                "  {:>2}. {:<30} ({})  centrality={:.6}  in={}  out={}\n",
                i + 1,
                node.name,
                location.dimmed(),
                score,
                in_deg,
                out_deg
            ));
        }

        out
    }

    /// Find tightly-coupled function clusters using weakly connected components.
    /// Uses BFS treating directed edges as undirected to find weakly connected components.
    pub fn communities(&self) -> String {
        // Build component membership via BFS (treating edges as undirected)
        let mut component_map: HashMap<NodeIndex, usize> = HashMap::new();
        let mut visited: HashSet<NodeIndex> = HashSet::new();
        let mut comp_id = 0;

        for start in self.graph.node_indices() {
            if visited.contains(&start) {
                continue;
            }
            // BFS treating edges as undirected
            let mut queue = VecDeque::new();
            queue.push_back(start);
            visited.insert(start);

            while let Some(node) = queue.pop_front() {
                component_map.insert(node, comp_id);
                // Follow outgoing edges
                for neighbor in self.graph.neighbors_directed(node, petgraph::Direction::Outgoing) {
                    if visited.insert(neighbor) {
                        queue.push_back(neighbor);
                    }
                }
                // Follow incoming edges (to treat as undirected)
                for neighbor in self.graph.neighbors_directed(node, petgraph::Direction::Incoming) {
                    if visited.insert(neighbor) {
                        queue.push_back(neighbor);
                    }
                }
            }
            comp_id += 1;
        }
        let components = comp_id;

        // Group nodes by component
        let mut groups: HashMap<usize, Vec<NodeIndex>> = HashMap::new();
        for (&idx, &comp) in &component_map {
            groups.entry(comp).or_default().push(idx);
        }

        // Sort groups by size descending
        let mut sorted_groups: Vec<_> = groups.into_iter().collect();
        sorted_groups.sort_by(|a, b| b.1.len().cmp(&a.1.len()));

        // Filter out singletons
        let clusters: Vec<_> = sorted_groups
            .into_iter()
            .filter(|(_, members)| members.len() > 1)
            .collect();

        if clusters.is_empty() {
            return format!(
                "{}\nNo function clusters found (all functions are isolated).\n",
                "=== Code Communities ===".bold()
            );
        }

        let mut out = format!(
            "{}\n{} connected components, {} cluster(s) with 2+ functions:\n",
            "=== Code Communities ===".bold(),
            components,
            clusters.len()
        );

        let max_show = 20;
        for (i, (_, members)) in clusters.iter().take(max_show).enumerate() {
            let files: HashSet<_> = members
                .iter()
                .map(|&idx| self.graph[idx].file.clone())
                .filter(|f| !f.is_empty())
                .collect();
            let mut sorted_files: Vec<_> = files.into_iter().collect();
            sorted_files.sort();

            out.push_str(&format!(
                "\n{}Cluster {} ({} functions, {} files):{}\n",
                "".clear(),
                i + 1,
                members.len(),
                sorted_files.len(),
                "".clear()
            ));

            // Show up to 10 function names
            let mut names: Vec<_> = members
                .iter()
                .map(|&idx| self.graph[idx].name.clone())
                .collect();
            names.sort();
            let display_count = 10.min(names.len());
            for name in &names[..display_count] {
                out.push_str(&format!("  - {}\n", name.cyan()));
            }
            if names.len() > display_count {
                out.push_str(&format!(
                    "  ... and {} more\n",
                    names.len() - display_count
                ));
            }

            if !sorted_files.is_empty() {
                let file_display = if sorted_files.len() <= 5 {
                    sorted_files.join(", ")
                } else {
                    format!(
                        "{}, ... and {} more",
                        sorted_files[..5].join(", "),
                        sorted_files.len() - 5
                    )
                };
                out.push_str(&format!("  Files: {}\n", file_display.dimmed()));
            }
        }

        if clusters.len() > max_show {
            out.push_str(&format!(
                "\n... and {} more small clusters\n",
                clusters.len() - max_show
            ));
        }

        out
    }

    /// Resolve a function name to a NodeIndex (fuzzy: prefix/suffix match if exact fails).
    fn resolve_node(&self, name: &str) -> Option<NodeIndex> {
        // Exact match
        if let Some(idxs) = self.name_to_idx.get(name) {
            return Some(idxs[0]);
        }

        // Try case-insensitive
        let lower = name.to_lowercase();
        for (k, idxs) in &self.name_to_idx {
            if k.to_lowercase() == lower {
                return Some(idxs[0]);
            }
        }

        // Try suffix match (e.g., "evaluate_condition" matches "module::evaluate_condition")
        for (k, idxs) in &self.name_to_idx {
            if k.ends_with(name) || k.ends_with(&format!("::{}", name)) {
                return Some(idxs[0]);
            }
        }

        None
    }
}

/// Load the code index for a repo and build the graph.
pub fn load_graph(repo: &str) -> Result<CodeGraph, String> {
    let path = dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".savants")
        .join("code-index")
        .join(format!("{}.json", repo));

    let data = std::fs::read_to_string(&path).map_err(|_| {
        format!(
            "No local index for '{}'. Run 'savants up' first to index the codebase.",
            repo
        )
    })?;

    let pr: ParseResult = serde_json::from_str(&data)
        .map_err(|e| format!("Corrupt index for '{}': {}", repo, e))?;

    Ok(CodeGraph::from_parse_result(&pr))
}
