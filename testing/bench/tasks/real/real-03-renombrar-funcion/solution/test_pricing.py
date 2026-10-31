import unittest

import pricing
from cart import cart_total
from invoice import invoice_line


class Pricing(unittest.TestCase):
    def test_calc_applies_the_discount(self):
        self.assertEqual(pricing.calculate_total(10, 3, 10), 27.0)

    def test_cart(self):
        self.assertEqual(cart_total([(10, 2), (5, 1)]), 25)

    def test_invoice(self):
        self.assertEqual(invoice_line(10, 3, 10), "3 x 10 = 27.00")


if __name__ == "__main__":
    unittest.main()
