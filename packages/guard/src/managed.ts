/**
 * Managed Guard Client — OPA/LaunchDarkly-style rule management.
 *
 * Fetches rule bundles from Savants cloud on startup.
 * Evaluates guards locally (zero network latency per check).
 * Reports events in batches (async, non-blocking).
 * Polls for rule changes (ETag-based, 304 on no change).
 */

import type { ASTNode } from './types.js';

// ============================================================
// TYPES
// ============================================================

export interface ManagedOptions {
  apiKey: string;
  apiUrl?: string;
  pollInterval?: number;   // ms, default: 30000
  batchSize?: number;      // default: 50
  batchInterval?: number;  // ms, default: 30000
}

export interface BundleRule {
  id: string;
  dsl: string;
  ast_json: string;
  priority: number;
}

export interface RuleBundle {
  version: number;
  hash: string;
  rules: BundleRule[];
  updated_at: string;
}

export interface ParsedManagedRule {
  id: string;
  dsl: string;
  condition: ASTNode;
  action: string;
  priority: number;
}

export interface GuardEvent {
  context_hash: string;
  action?: string;
  tool?: string;
  result: string;
  matched_rule?: string;
  bundle_version?: number;
  timestamp: string;
}

// ============================================================
// CLIENT
// ============================================================

export class ManagedGuardClient {
  private apiUrl: string;
  private apiKey: string;
  private pollInterval: number;
  private batchSize: number;
  private batchInterval: number;

  private currentHash: string = "";
  private bundleVersion: number = 0;
  private pollTimer: ReturnType<typeof setInterval> | null = null;
  private flushTimer: ReturnType<typeof setInterval> | null = null;
  private eventQueue: GuardEvent[] = [];

  constructor(options: ManagedOptions) {
    this.apiUrl = options.apiUrl || "https://api.savants.cloud";
    this.apiKey = options.apiKey;
    this.pollInterval = options.pollInterval || 30000;
    this.batchSize = options.batchSize || 50;
    this.batchInterval = options.batchInterval || 30000;
  }

  /**
   * Fetch the rule bundle from the cloud. Returns parsed rules.
   */
  async fetchBundle(): Promise<ParsedManagedRule[]> {
    const headers: Record<string, string> = {
      Authorization: `Bearer ${this.apiKey}`,
    };
    if (this.currentHash) {
      headers["If-None-Match"] = this.currentHash;
    }

    const resp = await fetch(`${this.apiUrl}/api/v1/guard/bundle`, { headers });

    if (resp.status === 304) {
      return []; // No change, caller keeps existing rules
    }

    if (!resp.ok) {
      throw new Error(`Failed to fetch bundle: ${resp.status}`);
    }

    const bundle = (await resp.json()) as RuleBundle;
    this.currentHash = bundle.hash;
    this.bundleVersion = bundle.version;

    return bundle.rules.map((r) => {
      const parsed = JSON.parse(r.ast_json) as { condition: ASTNode; action: string };
      return {
        id: r.id,
        dsl: r.dsl,
        condition: parsed.condition,
        action: parsed.action,
        priority: r.priority,
      };
    });
  }

  /**
   * Start polling for rule changes.
   * Calls onUpdate when new rules are available.
   */
  startPolling(onUpdate: (rules: ParsedManagedRule[]) => void): void {
    if (this.pollTimer) return;
    this.pollTimer = setInterval(async () => {
      try {
        const rules = await this.fetchBundle();
        if (rules.length > 0) {
          onUpdate(rules);
        }
      } catch {
        // Silent fail on poll — keep using cached rules
      }
    }, this.pollInterval);
  }

  /**
   * Stop polling.
   */
  stopPolling(): void {
    if (this.pollTimer) {
      clearInterval(this.pollTimer);
      this.pollTimer = null;
    }
  }

  /**
   * Queue a guard evaluation event.
   * Flushes automatically when batch is full.
   */
  reportEvent(event: Omit<GuardEvent, "bundle_version">): void {
    this.eventQueue.push({
      ...event,
      bundle_version: this.bundleVersion,
    });

    if (this.eventQueue.length >= this.batchSize) {
      this.flush().catch(() => {}); // Fire and forget
    }
  }

  /**
   * Start the periodic flush timer.
   */
  startFlushing(): void {
    if (this.flushTimer) return;
    this.flushTimer = setInterval(() => {
      if (this.eventQueue.length > 0) {
        this.flush().catch(() => {});
      }
    }, this.batchInterval);
  }

  /**
   * Flush all queued events to the cloud.
   */
  async flush(): Promise<void> {
    if (this.eventQueue.length === 0) return;

    const events = this.eventQueue.splice(0, this.eventQueue.length);

    try {
      await fetch(`${this.apiUrl}/api/v1/guard/events`, {
        method: "POST",
        headers: {
          Authorization: `Bearer ${this.apiKey}`,
          "Content-Type": "application/json",
        },
        body: JSON.stringify({ events }),
      });
    } catch {
      // On failure, put events back in queue for next flush
      this.eventQueue.unshift(...events);
    }
  }

  /**
   * Stop polling + flush remaining events.
   */
  async close(): Promise<void> {
    this.stopPolling();
    if (this.flushTimer) {
      clearInterval(this.flushTimer);
      this.flushTimer = null;
    }
    await this.flush();
  }
}
