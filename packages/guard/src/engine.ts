/**
 * Generic Schema-Driven FSM Engine.
 *
 * Loads FSM definitions from JSON schemas. Supports states (initial/active/terminal),
 * transitions with AST guards, timeouts, and universal fallbacks.
 *
 * The engine is a pure function: transition(schema, state, event, context) → new_state.
 * No side effects. State persistence is the caller's responsibility.
 *
 * Ported from Python fsm_engine.py — same semantics, same JSON schema format.
 */

import type {
  FSMSchemaDefinition,
  ASTNode,
  TransitionResult,
} from "./types";
import { evaluate } from "./ast";

// ============================================================
// FSM SCHEMA
// ============================================================

export class FSMSchema {
  readonly id: string;
  readonly version: string;
  readonly initialState: string;
  readonly states: Map<string, "initial" | "active" | "terminal">;
  readonly terminalStates: Set<string>;
  readonly events: Set<string>;
  readonly terminalResetEvent: string | null;
  readonly terminalResetTo: string | null;
  readonly universalFallbacks: Map<string, string>;
  readonly timeouts: Array<{ state: string; hours: number; event: string }>;

  private transitionIndex: Map<string, string>; // "state|event" → to_state

  constructor(definition: FSMSchemaDefinition) {
    this.id = definition.id || "unknown";
    this.version = definition.version || "1.0";
    this.initialState = definition.initial_state || "";
    this.events = new Set(definition.events || []);

    // Parse states
    this.states = new Map();
    this.terminalStates = new Set();
    for (const [name, info] of Object.entries(definition.states || {})) {
      const stateType = typeof info === "object" ? info.type || "active" : "active";
      this.states.set(name, stateType as "initial" | "active" | "terminal");
      if (stateType === "terminal") {
        this.terminalStates.add(name);
      }
    }

    // Parse transitions into an index
    this.transitionIndex = new Map();
    for (const t of definition.transitions || []) {
      this.transitionIndex.set(`${t.from}|${t.event}`, t.to);
    }

    // Universal fallbacks
    this.universalFallbacks = new Map();
    for (const fb of definition.universal_fallbacks || []) {
      this.universalFallbacks.set(fb.event, fb.to);
    }

    // Terminal reset
    const tr = definition.terminal_reset;
    this.terminalResetEvent = tr?.event || null;
    this.terminalResetTo = tr?.to || null;

    // Timeouts
    this.timeouts = (definition.timeouts || []).map((t) => ({
      state: t.state,
      hours: t.hours,
      event: t.event,
    }));
  }

  isValidState(state: string): boolean {
    return this.states.has(state);
  }

  isTerminal(state: string): boolean {
    return this.terminalStates.has(state);
  }

  getTransition(fromState: string, event: string): string | null {
    return this.transitionIndex.get(`${fromState}|${event}`) || null;
  }

  getValidEvents(state: string): string[] {
    if (this.isTerminal(state)) {
      return this.terminalResetEvent ? [this.terminalResetEvent] : [];
    }
    const events: Set<string> = new Set();
    for (const [key] of this.transitionIndex) {
      const [s, e] = key.split("|");
      if (s === state) events.add(e);
    }
    for (const [e] of this.universalFallbacks) {
      events.add(e);
    }
    return Array.from(events);
  }

  toJSON(): FSMSchemaDefinition {
    const states: Record<string, { type: string }> = {};
    for (const [name, type] of this.states) {
      states[name] = { type };
    }
    const transitions = [];
    for (const [key, to] of this.transitionIndex) {
      const [from, event] = key.split("|");
      transitions.push({ from, event, to });
    }
    const fallbacks = [];
    for (const [event, to] of this.universalFallbacks) {
      fallbacks.push({ event, to });
    }
    return {
      id: this.id,
      version: this.version,
      initial_state: this.initialState,
      states: states as FSMSchemaDefinition["states"],
      events: Array.from(this.events).sort(),
      transitions,
      universal_fallbacks: fallbacks,
      terminal_reset: this.terminalResetEvent
        ? { event: this.terminalResetEvent, to: this.terminalResetTo! }
        : undefined,
      timeouts: this.timeouts,
    };
  }
}

// ============================================================
// GUARD REGISTRY
// ============================================================

export class GuardRegistry {
  private guards: Map<string, ASTNode> = new Map();

  set(schemaId: string, state: string, event: string, astNode: ASTNode): void {
    this.guards.set(`${schemaId}|${state}|${event}`, astNode);
  }

  get(schemaId: string, state: string, event: string): ASTNode | null {
    return this.guards.get(`${schemaId}|${state}|${event}`) || null;
  }

  clear(schemaId?: string): void {
    if (schemaId) {
      for (const key of this.guards.keys()) {
        if (key.startsWith(`${schemaId}|`)) {
          this.guards.delete(key);
        }
      }
    } else {
      this.guards.clear();
    }
  }

  size(): number {
    return this.guards.size;
  }
}

// ============================================================
// TRANSITION FUNCTION (pure)
// ============================================================

/**
 * Compute the next state for an FSM instance.
 *
 * @param schema - The FSM schema definition
 * @param currentState - Current state string
 * @param event - Event string
 * @param context - Optional context for guard evaluation
 * @param guards - Optional guard registry
 * @returns TransitionResult with success, new state, and guard info
 */
export function transition(
  schema: FSMSchema,
  currentState: string,
  event: string,
  context?: Record<string, unknown>,
  guards?: GuardRegistry
): TransitionResult {
  const result: TransitionResult = {
    success: false,
    from_state: currentState,
    event,
    to_state: null,
    guard_blocked: false,
  };

  // Validate state
  if (!schema.isValidState(currentState)) {
    result.error = `Invalid state: ${currentState}`;
    return result;
  }

  // Terminal state — only reset works
  if (schema.isTerminal(currentState)) {
    if (event === schema.terminalResetEvent) {
      result.success = true;
      result.to_state = schema.terminalResetTo;
      return result;
    }
    result.error = `Terminal state: ${currentState}`;
    return result;
  }

  // Look up explicit transition
  const target = schema.getTransition(currentState, event);
  if (target !== null) {
    // Evaluate guard if context provided
    if (context && guards) {
      const guard = guards.get(schema.id, currentState, event);
      if (guard) {
        const guardResult = evaluate(guard, context);
        if (!guardResult) {
          result.guard_blocked = true;
          return result;
        }
      }
    }
    result.success = true;
    result.to_state = target;
    return result;
  }

  // Check universal fallbacks
  const fallbackTarget = schema.universalFallbacks.get(event);
  if (fallbackTarget) {
    result.success = true;
    result.to_state = fallbackTarget;
    return result;
  }

  result.error = `No transition from ${currentState} on ${event}`;
  return result;
}

/**
 * Simple transition — returns just the new state string or null.
 * Matches the Python fsm_engine.transition() signature.
 */
export function transitionSimple(
  schema: FSMSchema,
  currentState: string,
  event: string,
  context?: Record<string, unknown>,
  guards?: GuardRegistry
): string | null {
  const result = transition(schema, currentState, event, context, guards);
  return result.success ? result.to_state : null;
}

// ============================================================
// TIMEOUT CHECKER
// ============================================================

export function checkTimeouts(
  schema: FSMSchema,
  currentState: string,
  lastActivityTs: string | Date
): string | null {
  if (schema.isTerminal(currentState)) return null;

  let lastActivity: Date;
  try {
    lastActivity =
      lastActivityTs instanceof Date
        ? lastActivityTs
        : new Date(lastActivityTs);
  } catch {
    return null;
  }

  const now = new Date();
  const hoursElapsed =
    (now.getTime() - lastActivity.getTime()) / (1000 * 60 * 60);

  // Check timeouts (longest first)
  const applicable = schema.timeouts
    .filter((t) => t.state === currentState)
    .sort((a, b) => b.hours - a.hours);

  for (const { hours, event } of applicable) {
    if (hoursElapsed >= hours) {
      return event;
    }
  }

  return null;
}

// ============================================================
// SCHEMA REGISTRY
// ============================================================

const schemaCache = new Map<string, FSMSchema>();

export function loadSchema(definition: FSMSchemaDefinition): FSMSchema {
  const schema = new FSMSchema(definition);
  schemaCache.set(schema.id, schema);
  return schema;
}

export function getSchema(id: string): FSMSchema | null {
  return schemaCache.get(id) || null;
}

export function listSchemas(): string[] {
  return Array.from(schemaCache.keys());
}

export function clearSchemas(): void {
  schemaCache.clear();
}
