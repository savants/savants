/**
 * Type definitions for the schema-driven FSM engine.
 *
 * These types define the JSON interchange format for FSM schemas,
 * AST guard nodes, transitions, and profiles.
 */

// ============================================================
// FSM SCHEMA TYPES
// ============================================================

export interface FSMSchemaDefinition {
  id: string;
  version: string;
  initial_state: string;
  states: Record<string, { type: "initial" | "active" | "terminal" }>;
  events: string[];
  transitions: TransitionDefinition[];
  universal_fallbacks: FallbackDefinition[];
  terminal_reset?: { event: string; to: string };
  timeouts: TimeoutDefinition[];
}

export interface TransitionDefinition {
  from: string;
  event: string;
  to: string;
}

export interface FallbackDefinition {
  event: string;
  to: string;
}

export interface TimeoutDefinition {
  state: string;
  hours: number;
  event: string;
}

// ============================================================
// AST GUARD TYPES
// ============================================================

export type ASTNode =
  | LiteralNode
  | VarNode
  | CompareNode
  | AndNode
  | OrNode
  | NotNode
  | IfNode
  | ActionNode
  | RuleNode
  | RulesetNode
  | SequenceNode;

export interface LiteralNode {
  type: "literal";
  value: unknown;
}

export interface VarNode {
  type: "var";
  name: string;
}

export interface CompareNode {
  type: "compare";
  op: string;
  left: ASTNode;
  right: ASTNode;
}

export interface AndNode {
  type: "and";
  children: ASTNode[];
}

export interface OrNode {
  type: "or";
  children: ASTNode[];
}

export interface NotNode {
  type: "not";
  child: ASTNode;
}

export interface IfNode {
  type: "if";
  condition: ASTNode;
  then: ASTNode;
  else?: ASTNode;
}

export interface ActionNode {
  type: "action";
  name: string;
  params: Record<string, unknown>;
}

export interface RuleNode {
  type: "rule";
  name: string;
  when: ASTNode;
  then: ASTNode;
}

export interface RulesetNode {
  type: "ruleset";
  rules: ASTNode[];
}

export interface SequenceNode {
  type: "sequence";
  steps: ASTNode[];
}

// ============================================================
// GUARD & INSTANCE TYPES
// ============================================================

export interface GuardDefinition {
  schema_id: string;
  state: string;
  event: string;
  ast: ASTNode;
  description?: string;
}

export interface FSMInstance {
  schema_id: string;
  entity_id: string;
  current_state: string;
  context: Record<string, unknown>;
  last_activity_ts: string;
}

export interface TransitionResult {
  success: boolean;
  from_state: string;
  event: string;
  to_state: string | null;
  guard_blocked: boolean;
  error?: string;
}

export interface TransitionLogEntry {
  schema_id: string;
  entity_id: string;
  from_state: string;
  event: string;
  to_state: string | null;
  context: Record<string, unknown>;
  guard_result: string;
  timestamp: string;
}

// ============================================================
// PROFILE TYPES
// ============================================================

export interface ProfileDefinition {
  id: string;
  name: string;
  description: string;
  version: string;
  extends?: string;
  schemas: FSMSchemaDefinition[];
  guards: GuardDefinition[];
  required_adapters: string[];
}
