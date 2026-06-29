# Savants Diagnosis Accuracy Benchmark

## Methodology

Statistical target: 95% accuracy with 95% confidence, 3% margin of error.
Required sample size: 203 labeled test cases minimum.

## Test Case Format

Each test case is a JSON file in tests/accuracy_cases/:

```json
{
  "id": "case-001",
  "error_message": "vocator-frontend TypeError: t.split is not a function",
  "source": "sentry",
  "category": "frontend_crash",
  "confirmed_root_cause": {
    "file": "server/services/identity-verification.ts",
    "function": "performIdentityVerification",
    "description": "AI response stored without validateLLMOutput"
  },
  "confirmed_fix": {
    "pr": "#645",
    "commit": "abc123",
    "description": "Added validateLLMOutput to identity-verification.ts"
  },
  "repo": "talent-pipeline"
}
```

## Scoring Rubric

- Score 3 (Correct): Root cause file AND function match
- Score 2 (Partial): Root cause file matches, function differs
- Score 1 (Direction): Correct category (frontend/backend/infra) but wrong file
- Score 0 (Wrong): Incorrect root cause or no answer

## Accuracy Metrics

- Strict accuracy: score 3 / total >= 95%
- Useful accuracy: (score 2 + score 3) / total >= 98%
- Category accuracy: (score 1 + 2 + 3) / total >= 99%

## How to Collect 203 Cases

Sources ranked by reliability:

1. Sentry issues resolved by a merged PR (highest confidence)
   - Query: Sentry issues with state=resolved, linked to GitHub PR
   - The PR diff reveals the actual root cause file/function
   - Expected yield: 50-80 cases per customer with 6+ months of history

2. Jira tickets with status Done and linked commits
   - Cross-reference commit message with the ticket's error description
   - Expected yield: 30-50 cases per customer

3. Slack threads containing error reports followed by "fixed/resolved/deployed"
   - Match the error with the subsequent code change
   - Lower confidence (the fix might not be the root cause)
   - Expected yield: 20-30 cases per customer

4. Manually verified incidents
   - Engineer confirms "this was the root cause" after the fact
   - Highest confidence but doesn't scale
   - Expected yield: 10-20 cases per engagement

For launch: collect from 3-5 customer codebases to avoid overfitting to one repo.

## Running the Benchmark

```bash
savants benchmark --cases tests/accuracy_cases/ --repo talent-pipeline
```

Output:
```
203 cases evaluated
Score 3 (Correct):   194 (95.6%)
Score 2 (Partial):     5 (2.5%)
Score 1 (Direction):   3 (1.5%)
Score 0 (Wrong):       1 (0.5%)

Strict accuracy:    95.6% (target: 95%)  PASS
Useful accuracy:    98.0% (target: 98%)  PASS
Category accuracy:  99.5% (target: 99%)  PASS
```

## Continuous Monitoring

After launch, every diagnosis gets a thumbs up/down from the user.
Track rolling 30-day accuracy. Alert if it drops below 93%.
Add every confirmed wrong diagnosis as a new test case.
The benchmark suite grows over time and prevents regressions.

## Statistical Notes

- Wilson score interval for binomial proportion (not naive p/n)
- Stratify by error category to ensure accuracy isn't inflated by easy cases
- Report per-category accuracy separately
- Minimum 30 cases per category for statistical validity
