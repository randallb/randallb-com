# Billing handoff — November 16

Maya (billing): We still get empty daily exports. The script should print zero
so the reconciliation job doesn't need special handling.

Luis (operations): Promotional items can have a zero price, and zero quantities
can appear in canceled lines. Both are valid. Work in integer cents throughout;
we don't want floating point dollar conversion or rounding.

Maya: The sample's three pens at 150 cents and two books at 450 cents total
1350 cents. The script should take a file path rather than assuming a particular
filename. A colleague will inspect the result and tests after the first run.
