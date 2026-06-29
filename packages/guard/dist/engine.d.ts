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
import type { FSMSchemaDefinition, ASTNode, TransitionResult } from "./types";
export declare class FSMSchema {
    readonly id: string;
    readonly version: string;
    readonly initialState: string;
    readonly states: Map<string, "initial" | "active" | "terminal">;
    readonly terminalStates: Set<string>;
    readonly events: Set<string>;
    readonly terminalResetEvent: string | null;
    readonly terminalResetTo: string | null;
    readonly universalFallbacks: Map<string, string>;
    readonly timeouts: Array<{
        state: string;
        hours: number;
        event: string;
    }>;
    private transitionIndex;
    constructor(definition: FSMSchemaDefinition);
    isValidState(state: string): boolean;
    isTerminal(state: string): boolean;
    getTransition(fromState: string, event: string): string | null;
    getValidEvents(state: string): string[];
    toJSON(): FSMSchemaDefinition;
}
export declare class GuardRegistry {
    private guards;
    set(schemaId: string, state: string, event: string, astNode: ASTNode): void;
    get(schemaId: string, state: string, event: string): ASTNode | null;
    clear(schemaId?: string): void;
    size(): number;
}
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
export declare function transition(schema: FSMSchema, currentState: string, event: string, context?: Record<string, unknown>, guards?: GuardRegistry): TransitionResult;
/**
 * Simple transition — returns just the new state string or null.
 * Matches the Python fsm_engine.transition() signature.
 */
export declare function transitionSimple(schema: FSMSchema, currentState: string, event: string, context?: Record<string, unknown>, guards?: GuardRegistry): string | null;
export declare function checkTimeouts(schema: FSMSchema, currentState: string, lastActivityTs: string | Date): string | null;
export declare function loadSchema(definition: FSMSchemaDefinition): FSMSchema;
export declare function getSchema(id: string): FSMSchema | null;
export declare function listSchemas(): string[];
export declare function clearSchemas(): void;
