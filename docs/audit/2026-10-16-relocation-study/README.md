# Lectura que sobrevive a un parche que mueve código: diseño (16 oct 2026)

Base leída: addon `main` = `origin/main` = `19579f2` (0.8.14); plugin `main` = `origin/main` =
`888e12d1`. Ejecutable medido: copia del 16 oct, sha256 `4a220846…a64d` (la del README del
certificado). No se ha abierto ni leído el proceso del juego.

## 1. Premisas y restricciones

- Invariante: nunca leer con un perfil que no cuadra. Si falla cualquier comprobación, se
  devuelve `unsupported_build` y el log nombra la primera comprobación que falló. Se mantiene
  todo o nada, como hoy.
- Hoy el certificado comprueba offsets fijos del fichero. Medido con el JSON del 13 oct: en un
  parche como aquel **falla ya en la cabecera** (image size 0x2c48000 → 0x2c49000, y `.rdata`
  empieza una página más tarde). Además, 35 de los 37 rangos cambiaron de dirección y los 2
  que se quedaron en su sitio (`wallet:hash_lookup`, `mf:buff_lookup`) cambiaron bytes porque
  apuntan a `.rdata`, que sí se movió. Es decir, **0 de 37** habrían pasado.
- Los lectores no llaman a código del juego. Leen memoria siguiendo RVAs (vtables, funciones
  que comparan por dirección, la tabla hash, el índice TLS) y offsets de campo. Los rangos con
  digest sirven para demostrar que esos offsets y esa aritmética siguen siendo los del juego.
- Las RVAs están repartidas como constantes en `inventory.rs`, `bags.rs`, `wallet.rs` y
  `magic_find.rs` (unas 70 distintas, contadas con grep), además de `TLS_INDEX_RVA`, los límites de `.rdata`
  (`inventory.rs:344`) e `IMAGE_SIZE` en `executable.rs`. Hoy nada las recalcula.
- Sin conflictos con decisiones vigentes. El plugin acepta cualquier hash bien formado desde
  la 0.7.6, así que el diseño no lo toca. Tampoco cambia el protocolo.

## 2. Opciones

| | Qué es | Coste | Riesgo | Latencia tras un parche |
|---|---|---|---|---|
| (a) | El addon reubica solo: firmas normalizadas, búsqueda en el fichero y grafo de referencias | ~25–35 h | cambio amplio en los lectores; detalles de fail closed (§4) | ninguna (se resuelve en la carga) |
| (b) | Fuente que no dependa del binario | — | — | no existe |
| (c) | Script de recertificación en la máquina de David que publica una tabla firmada | (a) sin búsqueda + canal + firma: ~30–40 h | canal nuevo hacia el addon, gestión de claves | horas (hace falta una persona) |

- **(b)**: no reabro lo ya medido (Mumble y la caché de la API). Búsqueda de hoy: no aparece
  ninguna fuente nueva con inventario en tiempo real. Los addons de Nexus que enseñan bolsas
  (Gw2ItemSearch) usan la API oficial con clave. Nexus se presenta como cargador, sin datos de
  juego. DRF ya está descartado en el README.
- **(c) frente a (a)**: la parte cara, que es normalizar y hacer las comprobaciones cruzadas,
  es la misma en las dos. (c) solo se ahorra la búsqueda, que según las medidas de abajo es
  barata. Además, (c) no gana nada en distribución: Nexus ya actualiza la DLL desde GitHub
  Releases solo (≤30 min). Una tabla «solo datos» tarda lo mismo que una release de código, y
  encima exige firma y un canal nuevo (el addon solo habla por loopback con el plugin).
- **Recomendación: (a)**, con el generador de la tabla como herramienta offline (lote 1). Esa
  herramienta es además la vía semiautomática cuando (a) falle cerrado: hace en minutos lo que
  el 13 oct se hizo a mano y deja la recertificación en una release normal.

## 3. Cuánto del 13 oct se habría resuelto solo (medido)

Método (scripts de este directorio, §9): desensamblar cada rango en la copia de hoy, que según la recertificación
del 15 oct y el certificado del 16 contiene esos 37 rangos byte a byte en las RVAs del 13 oct.
Después, localizar los campos que se mueven con el código (rel32, disp32 rip-relativos,
desplazamientos relativos a la imagen tras `lea reg,[rip→image base]` y entradas de tablas de
saltos, incluidas las tablas internas del rango) y reconstruir los bytes de **antes** del 13 oct
con los `moved_fields` del JSON. **Los 37 dan el `old_sha256` del JSON**, así que el modelo de
campos es exactamente el de aquel día. Por último, se buscan los bytes viejos con esos campos
enmascarados en todo el `.text` de hoy, en cada offset de byte.

**Los 19 rangos que cambiaron de bytes:**

| Resultado | Nº | Rangos |
|---|---|---|
| Firma propia única, en el sitio correcto | **15** | hash_lookup, attribute_modifier_map, magic_find_case, capacity_counter_text, character_inventory_getter, balance_getter, bag_getter, bag_size_getter, slot_validity, modifier_sum, modifier_formula, modifier_value, buff_filter, item_definition_getter, item_factory_type_switch |
| Casa, pero no es única | 3 | buff_lookup (2), item_factory_bag_case (5), bag_item_constructor (3) |
| No se puede buscar (los 4 bytes se mueven) | 1 | item_factory_table_bag_entry |

Los 4 restantes se localizan por referencia desde un ancla ya localizada, y después se
verifican por su firma:
- buff_lookup, por el rel32 de modifier_value;
- table_bag_entry, como base de la tabla de type_switch + 12 (tipo 3, la bolsa);
- bag_case, por esa entrada;
- bag_item_constructor, por el rel32 de bag_case.

**Resultado: 15/19 solos y 19/19 con el grafo.** Los 18 que no cambiaron de bytes, aunque 16
sí cambiaron de dirección:
- 14 tienen firma única;
- la tabla hash es única en `.rdata` (8 alineado);
- 3 getters de 4 a 8 bytes (2, 2 y 5 coincidencias) salen del slot de su vtable.

Total: **37/37**.

**Resto de lo que leen los lectores (medido solo en la build de hoy, no en la del 13):**
- **14 funciones comparadas por dirección:**
  - 11 tienen firma única;
  - `item_instance_getter` (8 coincidencias) sale de un slot;
  - `null_stackable` (`33 C0 C3`, 1202 coincidencias) y `stack_quantity_getter` (62)
    salen de slots que hoy **no** se comprueban, así que hay que añadirlos a la tabla:
    - `bag_item +0x260 → 0x168aa0`;
    - `player_stats +0x260` o `conditional predicate C +0x28 → 0x168d10`.
- **18 vtables** (las 17 de los slots más `buff_node`):
  - 13 salen del `lea` de un constructor cuya firma es única;
  - las 2 vtables de predicado salen de sus slots, que solo coinciden en un sitio;
  - `bag_item` sale del `lea` dentro de `bag_item_constructor`;
  - `conditional item A/B`: sus constructores son gemelos (2 coincidencias cada uno) y se
    distinguen porque el `lea` siguiente instala la vtable de predicado A o B, ya localizada.
- **TLS**: AddressOfIndex sale de la cabecera, comprobado contra el getter de contexto
  (`0x9b1fb0`), que es destino de los rangos.

**Gemelos (por qué la unicidad sola no basta, medido):**
- todos los gemelos encontrados difieren de la verdadera en ≥1 campo reubicable;
- el de `buff_lookup` (en `0x431bc0`) se diferencia solo en el destino de un `call` (`0x257560`
  en vez de `0x2574f0`).

La comprobación de destinos (§4) los rechaza a todos.

**Mapa de deltas (medido, y descartado como regla):** de los 63 destinos que se movieron el
13 oct, en 15 el delta no coincide con ninguna de las anclas vecinas (p. ej. los
assert de `0x9daf40`, con delta 0x860). Validar destinos «por vecindad» habría fallado cerrado
casi siempre. Por eso la regla de abajo verifica el destino por su contenido.

## 4. Cómo se garantiza el fail closed (invariantes del diseño)

1. **Camino de identidad primero.** Si el fichero cumple el certificado de hoy en offsets
   fijos, se lee como hoy. La reubicación solo corre cuando ese camino falla.
2. **La búsqueda solo localiza lo que era único al certificar.** Cada ancla guarda cuántas
   veces casaba su firma en la build certificada. Solo se acepta por búsqueda la que casaba 1
   vez entonces y 1 vez ahora. Las demás, solo por derivación.
3. **Derivación con verificación.** Un ancla derivada (destino de un campo, entrada de tabla,
   slot de vtable o `lea` de constructor) tiene que casar su firma normalizada en el sitio
   derivado.
4. **Cada campo enmascarado tiene que cuadrar.** Si en la build certificada apuntaba a un
   ancla, ahora tiene que apuntar a la nueva posición de esa ancla. Si apuntaba a otra cosa,
   el destino tiene que tener la misma huella: tamaño y firma normalizada de la función
   (21 destinos con entrada en `.pdata`, 2.913 bytes en total; los 11 sin entrada son hojas o
   saltos internos, con límite registrado), o el mismo contenido si está en `.rdata` (35
   destinos). Un salto interno tiene que seguir interno y al mismo offset.
5. **Caminos redundantes deben coincidir.** Por ejemplo, la tabla hash por búsqueda y por
   los 6 campos rip que la nombran, o un getter por slot y entre sus coincidencias.
6. **Vtables** = destino del `lea` del constructor, y cada slot registrado tiene que valer
   `image_base + RVA nueva` de la función localizada.
7. **Nunca se enmascaran** offsets de campo, offsets de slot ni constantes. Si alguno cambia,
   no casa y se falla cerrado.
8. **Cabecera:** se siguen exigiendo AMD64, PE32+, image base 0x140000000 y las mismas 7
   secciones en orden. El tamaño y las posiciones pasan a leerse del propio fichero. La imagen
   en memoria tiene que tener el tamaño que dice el fichero, no una constante.
9. **Todo o nada.** Con un solo ancla sin resolver, `unsupported_build`, y el log dice cuál y
   por qué regla.

## 5. Qué sigue sin cubrirse (falla cerrado y pide recertificación humana con la herramienta)

- Cambios de código en un ancla: otra asignación de registros, inlining, un parámetro nuevo.
- Cambios de offset de campo, de layout de structs o de slots de vtable (un virtual nuevo).
- Un callee no ancla cuyo cuerpo cambie. Esto es **más estricto que hoy**: hoy solo se fija la
  dirección relativa. No se puede medir con qué frecuencia pasa, porque no hay builds viejas.
- Cambio de image base.
- **Riesgo que crece en frecuencia, no en naturaleza:** los offsets que hoy ya son supuestos
  (item +0x58, inventory +0xd0, definition +0x28, item context +0x38) y las dos reglas de
  punteros. Con (a) se leerán automáticamente más builds, así que habrá más ocasiones de que
  uno de ellos cambie sin que lo veamos. Lo propongo como lote 5.

## 6. Coste en el hilo del addon

- La búsqueda va sobre el **fichero**, dentro de la pasada de hash que ya existe (lecturas de
  64 KiB y rebanadas de 1 s, `SLICE`/`OVERDUE` de `executable.rs`). Una agujas de ≤32 bytes por
  ancla buscable (~70), en un índice por prefijo de 4 bytes, con un solapamiento de 31 bytes
  entre lecturas.
- Después viene una fase de lecturas puntuales (~150 lecturas de ≤4 KiB, <1 MiB): firmas,
  callees y slots. Es un paso `Unfinished` más, y hace falta `read_at` en `HashedFile`.
- Medido en Python: buscar las 37 firmas en `.text` (26 MB) tarda ~1,2 s, incluidos 37
  `objdump`. **Supuesto, sin medir:** en Rust el escaneo con índice por prefijo suma unas
  decenas de ms de CPU a la pasada de hash. El latido de 5 s no cambia, porque la búsqueda va
  dentro de las rebanadas. Se mide en el lote 3 con `perf.rs` o con el log.
- En el runtime no hay desensamblador: máscaras, agujas y huellas van precalculadas en la tabla.

## 7. ToS y detección

No cambia respecto a hoy. La búsqueda lee el ejecutable en disco, igual que el hash actual. Las
lecturas del proceso siguen siendo del mismo tipo (`ReadProcessMemory` acotado, sin llamadas,
hooks ni escrituras), solo que a las RVAs nuevas. No empeora nada que yo vea. Lo único que
añade es CPU en la carga.

## 8. Plan por lotes (cada uno de punta a punta por un agente)

| Lote | Resultado | Horas | Prueba que lo cierra |
|---|---|---|---|
| 1 | **Generador offline** (Python, `docs/audit/…`, a partir de estos scripts). A partir de la copia certificada y el certificado vigente emite `relocation_data.rs` con: anclas, agujas, máscaras, digest normalizado, coincidencias en la build certificada, aristas, slots nuevos, constructores y huellas de callees. También emite el JSON de evidencia. Incluye el modo «resolver contra otro exe», que es la recertificación semiautomática | 5–7 | Reproduce §3 (19/19 `old_sha256`, 15 únicas, 37/37) y, sobre la copia de hoy, da RVAs = constantes actuales. Negativos: un byte de offset de campo cambiado → falla esa ancla; el gemelo de `buff_lookup` → rechazado |
| 2 | **Resolver en `core/`** (host): escaneo en streaming dentro del `Collector`, fase de `read_at`, grafo, invariantes 1–9, salida `Layout` o primer fallo | 8–12 | PE sintético (como `certificate::synthetic`) con código movido por deltas → se resuelve. Negativos, uno por invariante: gemelo, campo a otro sitio, callee cambiado, slot cambiado, ambigüedad, offset de campo. Más un test `#[ignore]` con `TC_GW2_EXE` → `Layout` identidad. Cada garantía, vista fallar al romperla |
| 3 | **Lectores sobre `Layout`**: sustituir las ~70 RVAs, `TLS_INDEX_RVA`, límites de `.rdata` e `IMAGE_SIZE` | 6–10 | La suite de `core` en verde con `Layout` identidad. Los tests que fijan constantes pasan a fijar «identidad = perfil auditado». Un `Layout` con un delta sintético mueve las lecturas (test con memoria falsa) |
| 4 | **Adaptador Windows**: `read_at`, rebanadas de la fase 2, línea de log `certified by content (code-208704-r1), relocated …` / `not certified, <ancla> <regla>` | 2–4 | Build `x86_64-pc-windows-gnu` PASS. Test del modelo de rebanadas (latido ≤ 6,5 s en el peor caso, como hoy) |
| 5 | **Cubrir los offsets supuestos** (si el código del juego que los usa es localizable) | 3–5 | Rangos nuevos con digest que nombren +0x58/+0xd0/+0x28/+0x38. Negativo: rompiéndolos, falla |
| 6 | Release, README, recibo en `docs/audit/` | 2–3 | Gate del repo y canal publicado |

**Total: 26–41 h de agente, más la QA de David.** Camino crítico: 1 → 2 → 3 → 4 → 6. El 3 puede
empezar en paralelo al 2 en cuanto esté fijada la interfaz de `Layout`. El lote 1 ya sirve por
sí solo: es la recertificación en minutos cuando haya un parche.
Incertidumbres:
- con qué frecuencia cambian los callees, que mide si la huella resta cobertura;
- `read_at` en el adaptador;
- el tamaño real del cambio en los lectores (las constantes están repartidas por el código).

**Evidencia real pendiente (no la cierra ningún agente):**
- (i) David carga 0.8.15+ en la build actual y el log dice «certified by content», por el
  camino de identidad;
- (ii) el primer parche que mueva código dice «relocated» y se lee, o nombra el ancla que
  falla. Contrastarlo con el lote 1 sobre una copia de ese exe.

**Recomendación barata:** guardar una copia privada del `Gw2-64.exe` de cada build, fuera del
repo (~41 MB cada una). Sin builds viejas, nada de esto se puede probar de punta a punta antes
de un parche real.

## 9. Medido y supuesto

**Medido** (copia del 16 oct y JSON del 13/15 oct):
- reconstrucción 37/37 con `old_sha256`;
- coincidencias de cada firma;
- 15/19 y 19/19;
- vtables y slots;
- gemelos rechazables;
- mapa de deltas insuficiente (15/63);
- 21+11 callees y 35 destinos `.rdata`;
- 0/37 con el certificado de hoy.

**Supuesto:**
- que la unicidad en el `.text` de hoy equivale a la del `.text` del 13 oct, que ya no existe
  (los 37 rangos son los mismos, el resto no se puede comparar);
- que constructores, callees y las 14 funciones no cambiaron el 13 oct salvo reubicación. El
  JSON lo hace verosímil (19 cambiados, todos solo por campos reubicables), pero no lo prueba;
- el coste en Rust;
- que no hay otras RVAs fuera de las que encontró el grep.

**No probado:**
- ninguna lectura en el juego;
- Windows nativo;
- ningún exe movido real de punta a punta.

**Scripts** (en este directorio; necesitan `x86_64-w64-mingw32-objdump` y `normsig.py` al lado):
- `normsig.py`: reconstrucción, firmas y grafo;
- `ctor.py`: constructores;
- `twins.py`: gemelos;
- `callees.py`: destinos;
- `deltamap.py`: mapa de deltas;
- `refs.py` y `slotsof.py`: slots de los getters compartidos.

Se invocan con `python3 -I <script> <exe> <json 13 oct> [salida.json] [certificate_data.rs]`.

## 10. Decisiones pendientes de David

- Ninguna de producto para empezar. Solo una, menor: la tabla incrusta agujas de ≤32 bytes de
  código del juego (unos 2 KB en total) en un repo público. Hoy solo hay digests, salvo los
  bytes de guarda del wallet en un fixture. Si prefiere cero bytes, la alternativa es buscar
  por digest de ventanas alineadas, que es más lento. Hay que medirlo.
