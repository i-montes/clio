# Dinero en `shop`: de float a Decimal

Hoy todo el dinero son `float` y los totales se descuadran un centavo (por ejemplo con precios como 0.285 o 1.005). Hay que pasar TODO el paquete a `decimal.Decimal`. La estructura de archivos y los nombres públicos no cambian.

## Reglas

- Todo importe (precios, subtotal, descuento, impuesto, total, valores de `lines()`) es un `Decimal` con exactamente 2 decimales, redondeado SIEMPRE con `ROUND_HALF_UP` (0.285 -> 0.29, 0.005 -> 0.01). Nunca se calcula con `float`.
- `money.to_money(x)`: acepta `Decimal`, `int`, `str` y `float`. Un `float` se convierte con `Decimal(str(x))` (así 0.285 es exactamente 0.285). Devuelve el `Decimal` con 2 decimales. Un texto que no es un número finito (`"abc"`, `"NaN"`, `"Infinity"`) lanza `ValueError`; `None`, `bool` y otros tipos lanzan `TypeError`.
- `money.fmt(x)`: el importe como texto con exactamente 2 decimales y sin separadores de miles (`"1234.50"`, `"0.00"`).
- `catalog.CATALOG`: precios como `Decimal` (los mismos valores).
- `tax.RATES`: tasas como `Decimal`. `tax.tax_for(amount, category)` devuelve el impuesto de ese importe redondeado a 2 decimales; una categoría desconocida lanza `ValueError`.
- `Cart.add(sku, qty=1)` y `Cart.add_custom(name, unit_price, qty, category="standard")` (el precio pasa por `to_money`; `qty` debe ser un entero mayor que 0, si no `ValueError`). `Cart.lines()` devuelve tuplas `(name, unit_price, qty, category)`.
- Cálculo del carrito, por este orden:
  1. `line_total = unit_price * qty` (exacto). `subtotal()` es la suma de las líneas.
  2. Tramo de descuento según el subtotal: `>= 100.00` es 10 %, `>= 50.00` es 5 %, menos es 0 %. La comparación es exacta (49.99 no llega al 5 %).
  3. El descuento se calcula POR LÍNEA: `line_discount = to_money(line_total * porcentaje)`. `discount()` es la suma de los descuentos de línea.
  4. El impuesto se calcula POR LÍNEA sobre lo que queda tras el descuento: `tax_for(line_total - line_discount, categoría)`. `tax()` es la suma.
  5. `total() = subtotal() - discount() + tax()`.
- Un carrito vacío da `Decimal("0.00")` en todo.
- `invoice.render_invoice(cart)` mantiene su formato (una línea por producto y cuatro líneas finales `Subtotal:`, `Discount:`, `Tax:`, `Total:`), con los importes escritos con `money.fmt`.
