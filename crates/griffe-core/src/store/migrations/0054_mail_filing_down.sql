DROP TABLE mail_filing;
ALTER TABLE outbound_mail DROP COLUMN "references";
ALTER TABLE outbound_mail DROP COLUMN in_reply_to;
