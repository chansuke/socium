# money

Amounts come in as dollars (str or float) and are stored as whole cents.
Everywhere in this package, rounding to cents is half away from zero:
0.125 becomes 13 cents and -0.125 becomes -13.
