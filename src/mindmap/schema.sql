CREATE TABLE mindmaps(project_id TEXT PRIMARY KEY REFERENCES projects(id),version INTEGER NOT NULL DEFAULT 0);
CREATE TABLE mindmap_nodes(
 id TEXT PRIMARY KEY,project_id TEXT NOT NULL REFERENCES projects(id),alias TEXT,
 parent_id TEXT REFERENCES mindmap_nodes(id),position INTEGER NOT NULL,
 kind TEXT NOT NULL CHECK(kind IN ('text','markdown','issue','pr','notification')),
 title TEXT NOT NULL,body TEXT NOT NULL,reference TEXT,reference_project TEXT,
 created_at INTEGER NOT NULL,updated_at INTEGER NOT NULL,
 UNIQUE(project_id,alias)
);
CREATE INDEX mindmap_outline ON mindmap_nodes(project_id,parent_id,position,id);
CREATE UNIQUE INDEX mindmap_resource ON mindmap_nodes(project_id,kind,reference_project,reference) WHERE reference IS NOT NULL;
CREATE TABLE mindmap_links(
 source TEXT NOT NULL REFERENCES mindmap_nodes(id) ON DELETE CASCADE,
 target TEXT NOT NULL REFERENCES mindmap_nodes(id) ON DELETE CASCADE,
 kind TEXT NOT NULL,description TEXT,created_at INTEGER NOT NULL,
 PRIMARY KEY(source,target,kind),CHECK(source<>target)
);
CREATE INDEX mindmap_backlinks ON mindmap_links(target);
