import unittest

from pages import render_profile_page, render_search_box
from render import render_card


class RenderTests(unittest.TestCase):
    def test_card_has_the_fields(self):
        html = render_card("Ana", "Hola", "https://example.org/ana", ["py", "rust"])
        self.assertIn("<h2>Ana</h2>", html)
        self.assertIn('href="https://example.org/ana"', html)
        self.assertIn('<li class="tag">rust</li>', html)

    def test_search_box_keeps_a_plain_query(self):
        self.assertIn('value="cats"', render_search_box("cats"))

    def test_page_wraps_the_cards_once(self):
        page = render_profile_page("Team", [{"name": "Ana", "bio": "x", "website": "/ana"}])
        self.assertIn("<title>Team</title>", page)
        self.assertEqual(page.count('<div class="card"'), 1)


if __name__ == "__main__":
    unittest.main()
