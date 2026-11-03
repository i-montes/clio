import unittest

from cart import Cart
from invoice import render_invoice
from money import fmt, to_money
from tax import tax_for


class ShopTests(unittest.TestCase):
    def test_small_cart_has_no_discount(self):
        c = Cart()
        c.add("pen", 3)
        c.add("book")
        self.assertEqual(c.subtotal(), 16.29)
        self.assertEqual(c.discount(), 0.0)
        self.assertEqual(c.tax(), 1.54)
        self.assertEqual(c.total(), 17.83)

    def test_five_percent_tier(self):
        c = Cart()
        c.add("lamp", 2)
        c.add("pen")
        self.assertEqual(c.subtotal(), 50.10)
        self.assertEqual(c.discount(), 2.51)
        self.assertEqual(c.tax(), 9.04)
        self.assertEqual(c.total(), 56.63)

    def test_tax_for_rounds_to_cents(self):
        self.assertEqual(tax_for(3.30, "standard"), 0.63)

    def test_to_money_and_fmt(self):
        self.assertEqual(to_money("12.5"), 12.5)
        self.assertEqual(fmt(5), "5.00")

    def test_invoice_has_the_summary_lines(self):
        c = Cart()
        c.add("pen", 3)
        text = render_invoice(c)
        self.assertIn("Subtotal: 3.30", text)
        self.assertIn("Total: 3.93", text)

    def test_quantity_must_be_positive(self):
        with self.assertRaises(ValueError):
            Cart().add("pen", 0)


if __name__ == "__main__":
    unittest.main()
