-- Old schemas never had these guards. Remove them before reconstructing an
-- old database so DROP COLUMN validates the actual historical schema.
DROP TRIGGER dependency_notice_comment;
DROP TRIGGER dependency_notice_event;
DROP TRIGGER dependency_notice_steering;
DROP TRIGGER dependency_notice_mode;
DROP TRIGGER dependency_notice_delivery;
DROP VIEW obsolete_dependency_steering;
