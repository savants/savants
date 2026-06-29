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
import { evaluate } from "./ast";
// ============================================================
// FSM SCHEMA
// ============================================================
export class FSMSchema {
    id;
    version;
    initialState;
    states;
    terminalStates;
    events;
    terminalResetEvent;
    terminalResetTo;
    universalFallbacks;
    timeouts;
    transitionIndex; // "state|event" → to_state
    constructor(definition) {
        this.id = definition.id || "unknown";
        this.version = definition.version || "1.0";
        this.initialState = definition.initial_state || "";
        this.events = new Set(definition.events || []);
        // Parse states
        this.states = new Map();
        this.terminalStates = new Set();
        for (const [name, info] of Object.entries(definition.states || {})) {
            const stateType = typeof info === "object" ? info.type || "active" : "active";
            this.states.set(name, stateType);
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
    isValidState(state) {
        return this.states.has(state);
    }
    isTerminal(state) {
        return this.terminalStates.has(state);
    }
    getTransition(fromState, event) {
        return this.transitionIndex.get(`${fromState}|${event}`) || null;
    }
    getValidEvents(state) {
        if (this.isTerminal(state)) {
            return this.terminalResetEvent ? [this.terminalResetEvent] : [];
        }
        const events = new Set();
        for (const [key] of this.transitionIndex) {
            const [s, e] = key.split("|");
            if (s === state)
                events.add(e);
        }
        for (const [e] of this.universalFallbacks) {
            events.add(e);
        }
        return Array.from(events);
    }
    toJSON() {
        const states = {};
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
            states: states,
            events: Array.from(this.events).sort(),
            transitions,
            universal_fallbacks: fallbacks,
            terminal_reset: this.terminalResetEvent
                ? { event: this.terminalResetEvent, to: this.terminalResetTo }
                : undefined,
            timeouts: this.timeouts,
        };
    }
}
// ============================================================
// GUARD REGISTRY
// ============================================================
export class GuardRegistry {
    guards = new Map();
    set(schemaId, state, event, astNode) {
        this.guards.set(`${schemaId}|${state}|${event}`, astNode);
    }
    get(schemaId, state, event) {
        return this.guards.get(`${schemaId}|${state}|${event}`) || null;
    }
    clear(schemaId) {
        if (schemaId) {
            for (const key of this.guards.keys()) {
                if (key.startsWith(`${schemaId}|`)) {
                    this.guards.delete(key);
                }
            }
        }
        else {
            this.guards.clear();
        }
    }
    size() {
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
export function transition(schema, currentState, event, context, guards) {
    const result = {
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
export function transitionSimple(schema, currentState, event, context, guards) {
    const result = transition(schema, currentState, event, context, guards);
    return result.success ? result.to_state : null;
}
// ============================================================
// TIMEOUT CHECKER
// ============================================================
export function checkTimeouts(schema, currentState, lastActivityTs) {
    if (schema.isTerminal(currentState))
        return null;
    let lastActivity;
    try {
        lastActivity =
            lastActivityTs instanceof Date
                ? lastActivityTs
                : new Date(lastActivityTs);
    }
    catch {
        return null;
    }
    const now = new Date();
    const hoursElapsed = (now.getTime() - lastActivity.getTime()) / (1000 * 60 * 60);
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
const schemaCache = new Map();
export function loadSchema(definition) {
    const schema = new FSMSchema(definition);
    schemaCache.set(schema.id, schema);
    return schema;
}
export function getSchema(id) {
    return schemaCache.get(id) || null;
}
export function listSchemas() {
    return Array.from(schemaCache.keys());
}
export function clearSchemas() {
    schemaCache.clear();
}
