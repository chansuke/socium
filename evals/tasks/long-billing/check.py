import datetime
import sys

sys.path.insert(0, ".")
import legacy_billing as b

due = datetime.date(2026, 1, 1)
day = lambda n: due + datetime.timedelta(days=n)
business, personal = {"type": "business"}, {"type": "personal"}
invoice = {"due": due, "amount": 10_000}
for account, grace in ((business, 30), (personal, 14), ({}, 14)):
    assert b.late_fee(account, invoice, day(grace)) == 0, (account, grace)
    assert b.late_fee(account, invoice, day(grace + 1)) == 200, (account, grace)
    assert b.days_until_fee(account, invoice, day(0)) == grace + 1, (account, b.days_until_fee(account, invoice, day(0)))
    assert b.days_until_fee(account, invoice, day(grace + 5)) == 0
    assert b.projected_fees(account, [invoice, {**invoice, "paid": True}], day(grace + 1)) == 200
    assert b.projected_fees(account, [invoice], day(grace)) == 0
assert b.late_fee(business, {**invoice, "paid": True}, day(90)) == 0
for account in (business, personal):
    assert not b.reminder_due(invoice, day(13)) and b.reminder_due(invoice, day(14))
assert b.REMINDER_AFTER_DAYS == 14 and b.TAX_CODE_14 == "VAT-14"
assert b.tax_code({"country": "XX", "type": "business"}) == "VAT-14"
assert b.format_line_3("x", 150).endswith("1.50")
