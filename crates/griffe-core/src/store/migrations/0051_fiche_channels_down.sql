UPDATE contacts
SET email = COALESCE(
        email,
        (SELECT switchboard_email FROM clients WHERE clients.id = contacts.client_id)
    ),
    phone = COALESCE(
        phone,
        (SELECT switchboard_phone FROM clients WHERE clients.id = contacts.client_id)
    )
WHERE correspondent = 1;

ALTER TABLE outbound_mail DROP COLUMN recipient;
ALTER TABLE contacts DROP COLUMN civility;
ALTER TABLE contacts DROP COLUMN family_name;
ALTER TABLE contacts DROP COLUMN given_name;
ALTER TABLE clients DROP COLUMN switchboard_phone;
ALTER TABLE clients DROP COLUMN switchboard_email;
