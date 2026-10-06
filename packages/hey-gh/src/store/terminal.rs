use super::*;

impl Store {
    /// Queue retirement needs explicitly owned, validated terminal evidence.
    /// Project only its owner rather than decoding a full historical PR report.
    pub(crate) async fn terminal_pr_owner(
        &self,
        scope: &str,
        resource: &str,
        repository: &str,
        number: u64,
        node_id: &str,
        updated_at: Option<&str>,
    ) -> Result<Option<PrOwner>> {
        let (scope, resource, repository, node_id) = (
            scope.to_owned(),
            resource.to_owned(),
            repository.to_ascii_lowercase(),
            node_id.to_owned(),
        );
        let updated_at = updated_at.map(str::to_owned);
        self.read(move |conn| {
            let resource = resolve_pr_resource(conn, &scope, &resource)?;
            let generation = conn.query_row(
                "SELECT o.generation FROM snapshots s
                 JOIN source_owner o ON o.scope=s.scope AND o.resource=s.resource
                 JOIN pr_identity i ON i.scope=o.scope AND i.repository=o.repository AND i.pull_number=o.pull_number
                 JOIN snapshot_validation v ON v.scope=s.scope AND v.resource=s.resource
                 LEFT JOIN repository_generation g ON g.scope=o.scope AND g.repository=o.repository
                 WHERE s.scope=?1 AND s.resource=?2
                   AND o.repository=?3 AND o.pull_number=?4 AND o.node_id=?5
                   AND i.node_id=o.node_id AND i.generation=o.generation
                   AND o.generation=coalesce(g.generation,0)
                   AND v.validated_at_ms BETWEEN 1 AND ?6
                   AND json_extract(s.data,'$.pullRequest.id')=o.node_id
                   AND json_extract(s.data,'$.pullRequest.number')=o.pull_number
                   AND lower(json_extract(s.data,'$.pullRequest.repository.nameWithOwner'))=o.repository
                   AND json_extract(s.data,'$.pullRequest.state') IN ('CLOSED','MERGED')
                   AND (json_extract(s.data,'$.pullRequest.state')='MERGED' OR ?7 IS NULL
                        OR julianday(json_extract(s.data,'$.pullRequest.updatedAt'))>=julianday(?7))",
                params![scope,resource,repository,number,node_id,now_ms(),updated_at],
                |row| row.get::<_,u64>(0),
            ).optional().map_err(storage)?;
            Ok(generation.map(|generation| PrOwner {repository,number,node_id:Some(node_id),generation}))
        }).await
    }
}
