# Invoice export format

The first line is always `sku,quantity,unit_cents`. Further nonblank rows contain
an unquoted SKU, a nonnegative integer quantity, and a nonnegative integer unit
price in cents. There are no embedded commas or quoted fields in these exports.

The total is the sum of quantity multiplied by unit_cents. Print the integer
number of cents, followed by one newline, without currency symbols or labels.
Skip blank lines, including blank lines between records. An export with only
its header represents no purchases and totals zero. Repeated SKUs are separate
line items and should all be counted.
