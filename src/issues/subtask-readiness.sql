DROP VIEW issue_pickup_ready;
CREATE VIEW issue_pickup_ready AS
 SELECT i.project_id,i.number FROM issues i JOIN projects p ON p.id=i.project_id
 WHERE i.state='open' AND i.deleted_at IS NULL AND i.assignee IS NULL AND p.hidden_at IS NULL
 AND NOT EXISTS(
  WITH RECURSIVE family(number) AS (
   SELECT i.number UNION
   SELECT r.child_number FROM issue_subtasks r JOIN family f ON r.parent_number=f.number
    JOIN issues child ON child.project_id=r.project_id AND child.number=r.child_number
    WHERE r.project_id=i.project_id AND child.deleted_at IS NULL
  ) SELECT 1 FROM fleet_deferred_subtasks d JOIN family f ON json_extract(d.row_json,'$.parent_number')=f.number WHERE d.project_id=i.project_id
 )
 AND NOT EXISTS(SELECT 1 FROM worker_runs r WHERE r.project_id=i.project_id AND r.issue_number=i.number AND r.finished_at IS NULL)
 AND NOT EXISTS(SELECT 1 FROM worker_runs r WHERE r.id=(
  SELECT latest.id FROM worker_runs latest WHERE latest.project_id=i.project_id AND latest.issue_number=i.number AND latest.finished_at IS NOT NULL
  ORDER BY latest.finished_at DESC,latest.started_at DESC,latest.id DESC LIMIT 1)
  AND r.state!='completed' AND r.retry_allowed=0 AND (
   r.state='infrastructure_blocked' OR r.summary LIKE 'Codex needs input or approval:%'
   OR r.finished_at+min(300000,30000*(1<<min(4,(
    SELECT count(*)-1 FROM worker_runs failures WHERE failures.project_id=i.project_id AND failures.issue_number=i.number AND failures.finished_at IS NOT NULL AND failures.state NOT IN ('completed','infrastructure_blocked')
   ))))>CAST(unixepoch('subsec')*1000 AS INTEGER)))
 AND NOT EXISTS(
  WITH RECURSIVE descendants(number) AS (
   SELECT r.child_number FROM issue_subtasks r JOIN issues child ON child.project_id=r.project_id AND child.number=r.child_number
    WHERE r.project_id=i.project_id AND r.parent_number=i.number AND child.deleted_at IS NULL
   UNION ALL
   SELECT r.child_number FROM issue_subtasks r JOIN descendants d ON r.parent_number=d.number
    JOIN issues child ON child.project_id=r.project_id AND child.number=r.child_number
    WHERE r.project_id=i.project_id AND child.deleted_at IS NULL
  ) SELECT 1 FROM descendants d JOIN issues child ON child.project_id=i.project_id AND child.number=d.number WHERE child.state IN ('open','blocked')
 );
