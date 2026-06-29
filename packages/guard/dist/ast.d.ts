/**
 * AST Engine — safe expression evaluator for FSM guard conditions.
 *
 * Evaluates JSON AST nodes against a context dictionary.
 * NO eval(), NO Function constructor, NO code injection.
 * All evaluation is done by walking the AST tree.
 *
 * Ported from Python ast_engine.py — same semantics, same operators.
 */
import type { ASTNode } from "./types";
/**
 * Evaluate an AST node against a context dictionary.
 * Returns the result of the evaluation (boolean for conditions,
 * any value for literals/variables, action dicts for actions).
 */
export declare function evaluate(node: ASTNode | Record<string, unknown> | null | undefined, context: Record<string, unknown>): unknown;
export declare const var_: (name: string) => ASTNode;
export declare const lit: (value: unknown) => ASTNode;
export declare const compare: (field: string, op: string, value: unknown) => ASTNode;
export declare const and_: (...conditions: ASTNode[]) => ASTNode;
export declare const or_: (...conditions: ASTNode[]) => ASTNode;
export declare const not_: (condition: ASTNode) => ASTNode;
export declare const action: (name: string, params?: Record<string, unknown>) => ASTNode;
