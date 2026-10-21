DROP VIEW issue_pickup_ready;
CREATE VIEW issue_pickup_ready AS
 -- surviving_attempt_protection
 -- ready_dependencies_fast: workers attest PR readiness; no CI gate.
 -- explicit_dependencies_only: grouping does not imply sibling dependencies.
 SELECT i.project_id,i.number FROM issues i JOIN projects p ON p.id=i.project_id
 WHERE i.job_run_id IS NULL AND i.attempt_hold IS NULL AND i.state='open' AND i.deleted_at IS NULL AND i.assignee IS NULL AND p.hidden_at IS NULL
 -- project_pause: applies to every worker and fleet allocation path.
 AND NOT EXISTS(SELECT 1 FROM project_settings s WHERE s.project_id=i.project_id AND s.paused=1)
 AND (NOT EXISTS(SELECT 1 FROM fleet_deferred_subtasks pending WHERE pending.project_id=i.project_id) OR NOT EXISTS(
  WITH RECURSIVE family(number) AS (
   SELECT i.number UNION
   SELECT r.child_number FROM issue_subtasks r JOIN family f ON r.parent_number=f.number
    JOIN issues child ON child.project_id=r.project_id AND child.number=r.child_number
    WHERE r.project_id=i.project_id AND child.deleted_at IS NULL
  ) SELECT 1 FROM fleet_deferred_subtasks d JOIN family f ON json_extract(d.row_json,'$.parent_number')=f.number WHERE d.project_id=i.project_id
 ))
 AND (i.blockers='[]' OR NOT EXISTS(SELECT 1 FROM json_each(i.blockers) link LEFT JOIN issues dependency ON dependency.project_id=i.project_id AND dependency.number=link.value WHERE dependency.number IS NULL OR (dependency.deleted_at IS NULL AND dependency.state NOT IN ('closed','ready'))))
 AND NOT EXISTS(SELECT 1 FROM worker_runs r WHERE r.project_id=i.project_id AND r.issue_number=i.number AND r.finished_at IS NULL)
 -- scoped_worker_retry_history: constrain each history branch before UNION.
 AND NOT coalesce((SELECT r.state!='completed' AND r.retry_allowed=0 AND (
   r.summary LIKE 'Codex needs input or approval:%'
   OR coalesce(r.retry_at,r.finished_at+300000)>CAST(unixepoch('subsec')*1000 AS INTEGER))
  FROM ({scoped_retry_history}) r
  ORDER BY r.finished_at DESC,r.started_at DESC,r.id DESC LIMIT 1),0)
 AND (NOT EXISTS(SELECT 1 FROM issue_subtasks edge WHERE edge.project_id=i.project_id AND edge.parent_number=i.number) OR NOT EXISTS(
  WITH RECURSIVE descendants(number) AS (
   SELECT r.child_number FROM issue_subtasks r JOIN issues child ON child.project_id=r.project_id AND child.number=r.child_number
    WHERE r.project_id=i.project_id AND r.parent_number=i.number AND child.deleted_at IS NULL
   UNION ALL
   SELECT r.child_number FROM issue_subtasks r JOIN descendants d ON r.parent_number=d.number
    JOIN issues child ON child.project_id=r.project_id AND child.number=r.child_number
    WHERE r.project_id=i.project_id AND child.deleted_at IS NULL
  ) SELECT 1 FROM descendants d JOIN issues child ON child.project_id=i.project_id AND child.number=d.number WHERE child.state IN ('open','blocked')
 ));
