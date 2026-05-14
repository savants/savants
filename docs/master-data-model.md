# Savants Master Data Model

## Purpose
Define every entity, relationship, and state that savants must track to answer
any question a startup or enterprise engineering org would ask about their
software development lifecycle.

## Design Principles
1. Every entity has a canonical ID that links across platforms
2. Relationships are directional with timestamps (who did what when)
3. States are tracked as transitions, not just current values
4. All data maps to the graph: entities = nodes, relationships = edges
5. Metrics are computed from raw data, not stored separately

---

## Entities

### 1. Developer
Source: GitHub, Jira, Linear, Slack, Git

| Field | Source | Description |
|-------|--------|-------------|
| id | savants | Canonical ID (links GitHub + Jira + Slack identities) |
| github_login | GitHub | GitHub username |
| jira_account_id | Jira | Jira account ID |
| linear_id | Linear | Linear user ID |
| slack_id | Slack | Slack member ID |
| email | Git/GitHub | Primary email |
| name | Git | Display name |
| team | Jira/Linear | Team or squad |
| role | org | IC / lead / manager |
| first_commit | Git | Date of first commit |
| last_active | computed | Most recent action across all platforms |

### 2. Repository
Source: GitHub

| Field | Source | Description |
|-------|--------|-------------|
| id | GitHub | org/repo |
| default_branch | GitHub | main/master/develop |
| language | GitHub | Primary language |
| topics | GitHub | Tags/labels |
| team_owners | GitHub CODEOWNERS | Teams that own this repo |
| ci_provider | GitHub Actions | CI/CD system |
| deploy_target | savants/k8s | Where this deploys to |
| graph_node_count | savants | Functions indexed |

### 3. Pull Request
Source: GitHub

| Field | Source | Description |
|-------|--------|-------------|
| id | GitHub | PR number |
| repo | GitHub | Repository |
| author | GitHub | Developer who created it |
| title | GitHub | PR title |
| body | GitHub | PR description |
| branch_head | GitHub | Source branch |
| branch_base | GitHub | Target branch |
| ticket_id | parsed from title/branch | Linked Jira/Linear ticket |
| state | GitHub | open / closed |
| draft | GitHub | Boolean |
| merged | GitHub | Boolean |
| additions | GitHub | Lines added |
| deletions | GitHub | Lines removed |
| changed_files | GitHub | Number of files changed |
| commits | GitHub | Number of commits |
| created_at | GitHub | Timestamp |
| first_review_at | GitHub | When first review was submitted |
| approved_at | GitHub | When approved |
| merged_at | GitHub | When merged |
| closed_at | GitHub | When closed (if not merged) |
| time_to_first_review | computed | first_review_at - created_at |
| time_to_merge | computed | merged_at - created_at |
| review_rounds | computed | Number of request_changes before approval |
| reverted | computed | Was this PR later reverted? |
| deploy_id | savants | Linked deployment (if detected) |
| blast_radius | savants graph | Functions affected by this PR |

**PR State Machine:**
```
draft -> open -> review_requested -> changes_requested -> approved -> merged
                                  -> approved -> merged
                     open -> closed (without merge)
                     merged -> reverted (new PR that reverts)
```

### 4. Review
Source: GitHub

| Field | Source | Description |
|-------|--------|-------------|
| id | GitHub | Review ID |
| pr_id | GitHub | PR this review is on |
| reviewer | GitHub | Developer who reviewed |
| state | GitHub | pending / commented / approved / changes_requested / dismissed |
| body | GitHub | Review comment text |
| submitted_at | GitHub | Timestamp |
| comment_count | GitHub | Number of inline comments |
| is_rubber_stamp | computed | approved with 0 comments and < 2 min review time |

### 5. Ticket (Jira Issue / Linear Issue)
Source: Jira, Linear

| Field | Source | Description |
|-------|--------|-------------|
| id | Jira/Linear | Ticket ID (PROJ-123) |
| source | savants | "jira" or "linear" |
| title | Jira/Linear | Summary |
| description | Jira/Linear | Full description |
| type | Jira/Linear | story / bug / task / epic / subtask |
| status | Jira/Linear | Current status |
| priority | Jira/Linear | P0-P4 or urgent/high/medium/low |
| story_points | Jira/Linear | Estimated effort |
| assignee | Jira/Linear | Current assignee (Developer) |
| reporter | Jira/Linear | Who created it |
| sprint | Jira/Linear | Current sprint |
| epic | Jira/Linear | Parent epic |
| labels | Jira/Linear | Tags |
| created_at | Jira/Linear | Timestamp |
| started_at | Jira/Linear | When moved to "In Progress" |
| completed_at | Jira/Linear | When moved to "Done" |
| cycle_time | computed | completed_at - started_at |
| lead_time | computed | completed_at - created_at |
| linked_prs | GitHub | PRs that reference this ticket |
| linked_deploys | savants | Deploys that include this ticket |
| linked_errors | Sentry | Errors in code changed by this ticket |
| time_logged | Jira | Manual time tracking (if used) |
| subtask_count | Jira/Linear | Number of subtasks |
| subtasks_done | Jira/Linear | Subtasks completed |
| blocked_by | Jira/Linear | Blocking tickets |
| blocks | Jira/Linear | Tickets this blocks |

**Ticket State Machine (Jira):**
```
Backlog -> To Do -> In Progress -> In Review -> Done
                 -> Blocked -> In Progress
                                In Review -> In Progress (changes requested)
                                          -> Done
```

**Ticket State Machine (Linear):**
```
Backlog -> Todo -> In Progress -> In Review -> Done -> Cancelled
                -> Blocked
```

### 6. Sprint
Source: Jira, Linear

| Field | Source | Description |
|-------|--------|-------------|
| id | Jira/Linear | Sprint ID |
| name | Jira/Linear | Sprint name |
| goal | Jira/Linear | Sprint goal text |
| start_date | Jira/Linear | Start |
| end_date | Jira/Linear | End |
| committed_points | computed | Points in sprint at start |
| completed_points | computed | Points done by end |
| carry_over_points | computed | committed - completed |
| velocity | computed | completed_points |
| tickets | Jira/Linear | Tickets in this sprint |
| scope_change | computed | Tickets added/removed mid-sprint |

### 7. Deployment
Source: GitHub Actions, ArgoCD, k8s, savants agent

| Field | Source | Description |
|-------|--------|-------------|
| id | savants | Unique deploy ID |
| repo | GitHub | Repository deployed |
| environment | CI/k8s | staging / production |
| commit_sha | GitHub | Deployed commit |
| tag | GitHub | Release tag (if any) |
| triggered_by | GitHub | Developer or CI |
| status | CI/k8s | pending / running / success / failed / rolled_back |
| started_at | CI | Timestamp |
| completed_at | CI | Timestamp |
| duration_sec | computed | Time to deploy |
| tickets_included | computed | Jira tickets in commits since last deploy |
| prs_included | computed | PRs merged since last deploy |
| errors_after | Sentry | New errors within 1h of deploy |
| rollback | computed | Was this deploy rolled back? |
| change_failure | computed | Did this deploy cause errors? |

### 8. Error / Incident
Source: Sentry, PagerDuty, savants agent

| Field | Source | Description |
|-------|--------|-------------|
| id | Sentry | Issue ID |
| title | Sentry | Error message |
| level | Sentry | error / warning / fatal |
| first_seen | Sentry | First occurrence |
| last_seen | Sentry | Most recent |
| count | Sentry | Total occurrences |
| culprit | Sentry | Function that errored |
| file | Sentry | File path |
| release | Sentry | Which deploy introduced it |
| assignee | Sentry | Who's assigned |
| status | Sentry | unresolved / resolved / ignored |
| linked_ticket | Sentry/Jira | Ticket created for this error |
| linked_deploy | savants | Deploy that introduced it |
| linked_function | savants graph | Graph node for the culprit function |
| mttr | computed | Time from first_seen to resolved |

### 9. Function (Code Entity)
Source: savants parser, graph

| Field | Source | Description |
|-------|--------|-------------|
| id | savants | Graph node ID |
| name | parser | Function name |
| file_path | parser | File location |
| line_start | parser | Start line |
| line_end | parser | End line |
| language | parser | TypeScript, Python, Rust, etc. |
| params | parser | Parameter types |
| exported | parser | Is it exported/public? |
| complexity | parser | Cyclomatic complexity (future) |
| test_coverage | CI | Lines covered by tests (future) |
| callers | graph | Functions that call this |
| callees | graph | Functions this calls |
| last_modified | git | Last commit that changed this function |
| modified_by | git | Developers who changed it |
| error_rate | Sentry | How often this function errors |
| change_frequency | git | How often it's modified |
| hotspot_score | computed | change_frequency * error_rate |

---

## Relationships (Graph Edges)

| Edge Type | From | To | Description |
|-----------|------|-----|-------------|
| AUTHORED | Developer | PR | Created the PR |
| REVIEWED | Developer | PR | Submitted a review |
| ASSIGNED | Developer | Ticket | Assigned to work on |
| COMMITTED | Developer | Commit | Authored the commit |
| REFERENCES | PR | Ticket | PR title/branch contains ticket ID |
| INCLUDES | Deploy | PR | PRs merged in this deploy |
| INCLUDES | Deploy | Ticket | Tickets completed in this deploy |
| INTRODUCED | Deploy | Error | Error first seen after this deploy |
| CALLS | Function | Function | Static call relationship |
| IMPORTS | Function | Function | Import dependency |
| MODIFIES | PR | Function | PR changes this function |
| CAUSES | Error | Function | Error originates from this function |
| BLOCKS | Ticket | Ticket | Dependency relationship |
| BELONGS_TO | Ticket | Sprint | In this sprint |
| BELONGS_TO | Ticket | Epic | Under this epic |
| OWNS | Developer | Repository | CODEOWNERS |
| MENTIONS | Slack Message | Ticket | Slack thread about a ticket |
| RESOLVES | PR | Error | PR fixes this Sentry issue |

---

## Computed Metrics

### DORA Metrics (per team/repo, per week/month)
1. **Deployment Frequency**: deploys per week
2. **Lead Time for Changes**: median(PR merge to production deploy)
3. **Change Failure Rate**: deploys with errors / total deploys
4. **Mean Time to Recovery**: median(error first_seen to resolved)

### Developer Metrics (per developer, per sprint)
1. **PR Velocity**: PRs merged per week
2. **Lines per PR**: median lines changed per PR
3. **Review Turnaround**: median time from review request to first review
4. **Review Depth**: median comments per review (rubber-stamp detection: approved, 0 comments, < 2min)
5. **Story Points Velocity**: points completed per sprint
6. **Lines per Point**: total lines changed / total points
7. **Ticket Cycle Time**: median time from In Progress to Done
8. **Carry-Over Rate**: % of committed points not completed in sprint
9. **Code Hotspot Score**: functions modified by this developer that have high error rates

### Team Metrics (per team/repo, per sprint)
1. **Sprint Velocity Trend**: velocity over last 6 sprints
2. **Scope Creep**: tickets added mid-sprint / committed tickets
3. **PR Review Coverage**: % of PRs with >= 1 non-author review
4. **Deploy-to-Error Correlation**: which deploys caused errors
5. **Bus Factor**: how many developers have committed to each area of code

---

## Platform State Ingestion

### GitHub Webhooks (real-time)
- `pull_request` (opened, closed, merged, review_requested, ready_for_review)
- `pull_request_review` (submitted, dismissed)
- `push` (commits)
- `deployment_status` (success, failure)
- `check_run` (completed)

### Jira Webhooks (real-time)
- `jira:issue_created`
- `jira:issue_updated` (status change, assignee change, sprint change, points change)
- `sprint_started`, `sprint_closed`

### Linear Webhooks (real-time)
- `Issue` (created, updated - state, assignee, estimate)
- `Cycle` (started, completed)

### Sentry Webhooks (real-time)
- `issue.created` (new error group)
- `issue.resolved`
- `event.alert` (threshold crossed)

### Polling Fallback (if webhooks not configured)
- GitHub: poll /events, /pulls, /commits every 5 min
- Jira: poll /search with JQL every 5 min
- Linear: poll GraphQL every 5 min
- Sentry: poll /issues every 5 min

---

## Implementation Priority

### Phase 1: What we have (partially)
- [x] Developer identity (GitHub login only)
- [x] PR basics (number, title, state, author)
- [x] Commit basics (sha, message, author, date)
- [x] Ticket basics (Jira/Linear ID, title, status)
- [x] Function graph (name, file, callers, callees)
- [x] Error basics (Sentry title, culprit, count)
- [x] developer_report tool (PR + story points cross-ref)

### Phase 2: Complete the entity model
- [ ] PR: additions/deletions/changed_files on every PR (not just search results)
- [ ] PR: review data (reviewer, state, timestamp, comment_count)
- [ ] Ticket: story_points, sprint, epic, cycle_time
- [ ] Deploy: detect from GitHub Actions + k8s, link to PRs
- [ ] Developer: unified identity across GitHub + Jira + Linear + Slack

### Phase 3: Relationships
- [ ] PR -> Ticket linking (parse from title/branch, store as edge)
- [ ] Deploy -> PR linking (commits between deploys)
- [ ] Deploy -> Error linking (Sentry errors after deploy)
- [ ] PR -> Function linking (which functions did this PR modify)
- [ ] Error -> Function linking (Sentry culprit -> graph node)

### Phase 4: Computed Metrics
- [ ] DORA metrics dashboard
- [ ] Sprint velocity tracking
- [ ] Developer profile page
- [ ] Review health dashboard
- [ ] Hotspot detection (high change + high error areas)

### Phase 5: Real-time Ingestion
- [ ] GitHub webhook receiver
- [ ] Jira webhook receiver
- [ ] Linear webhook receiver
- [ ] Sentry webhook receiver
- [ ] Event-driven graph updates (no polling)
