# Cutmary — relevo de desarrollo

## Coordinación en main

Antigravity lleva la ventana del menú y el alfa de OpenGL en AppKit. Claude lleva la latencia del editor, el muestreador nativo y Metal. Codex lleva CoreVideo, la auditoría de importación y la documentación. El agente de Colab mantiene `dev`. Los tres colaboradores locales publican en `main` con commits pequeños, tras `git pull --rebase origin main`.

Los cinco parches se regeneran desde el árbol compartido de Rosemary. Antes de cada commit se aplican en orden sobre `411f2844cbd44754f727c2ebd22d7a168a506097` y se compara el árbol completo. La compilación de referencia es `nice -n 10 cargo build --release -j 6`.

## Registro de main

- Codex retiró una copia duplicada de las instrucciones históricas de Colab y comprobó que queda un único apartado de resultados CachyOS.
- En `c662b12`, Codex actualizó el README con la instalación del launcher, sus tres vías de distribución previstas y los resultados en vivo del editor. Comprobó las recetas y los índices públicos de Releases y AUR. La instalación de los paquetes publicados sigue pendiente.
- `cc55fac` retiró dos constantes duplicadas. `e0ee810` incorporó trazas de latencia y `b6bb3e8` registró la prueba del menú en Wayland. Los tres commits figuran a nombre de Clio en Git. Las medidas y pendientes de esa ronda están al final.

## Historial de Colab

### Estado en Colab (CapCut 9.8.0.4799 x86_64)

- **Commit base de Rosemary**: `411f2844cbd44754f727c2ebd22d7a168a506097`
- **Hasta dónde llega CapCut en Colab (Xvfb + openbox + Lavapipe/llvmpipe)**:
  - Arranca completo, carga ~198 hilos y ~2 GiB RSS.
  - Renderiza la **ventana principal de inicio (Home)** con barra de título macOS y el diálogo modal **"Terms of Service and Privacy Policy"** (`Agree and continue`).
  - Resuelto el crash frecuente a los ~7s por corrupción de heap / double-free (Qt 6.2.2 `QRhiMetal` hacía `objc_release` sobre `[NSString stringWithUTF8String:"main0"]`).
  - Resuelto el crash de CEF (`SIGSEGV at 0x0` desde `Chromium Embedded Framework+0x46093c6`) implementando `posix_spawnattr_set_csm_np`; los procesos helper de CEF ya arrancan sin crashear.
  - **En curso en Colab**:
    - Pulsar **"Agree and continue"** -> **"+ New project"** para entrar al **Editor de Vídeo** y arreglar los siguientes selectores/símbolos/crashes.

---

## Qué ha cambiado en `dev` desde `main` (`9bbb3a4`)

1. **`fix(appkit): NSColorSpace` (`patches/04-appkit-coregraphics-globals.patch`)**:
   - Implementados `-[NSColorSpace initWithICCProfileData:]`, `-[NSColorSpace initWithCGColorSpace:]`, `ICCProfileData`, `colorSpaceModel`, `numberOfColorComponents`, `CGColorSpace`, y `dealloc`.
   - Espacios compartidos inmortales separados para Gray (`model=0, components=1`, `CGColorSpaceCreateDeviceGray`) y CMYK (`model=2, components=4`) frente a RGB (`model=1, components=3`).
   - `initWithICCProfileData:` devuelve `nil` (liberando `receiver`) si los datos son `nil`, vacíos o no son `CFData`, igual que en macOS.
2. **`fix(corefoundation): CFMachPortCreateRunLoopSource` (`patches/02-cf-foundation-iokit-coretext.patch`)**:
   - Exportado `CFMachPortCreateRunLoopSource` (evita `SIGSEGV at 0x0` en `libqcocoa.dylib+0x323f9`).
   - Desmontaje de `libqcocoa.dylib+0x31ec0..0x3240c` confirmó que `CGEventTapCreateForPid` devuelve `NULL` y Qt realmente usa `CVDisplayLink` + `CFRunLoopSource` v0 para updates/timers; para puertos no-NULL hay un fallback inerte marcado `STUB:`.
3. **`fix(foundation): NSData writeToFile/writeToURL` (`patches/02-cf-foundation-iokit-coretext.patch`)**:
   - Implementados `writeToFile:options:error:`, `writeToURL:atomically:`, `writeToURL:options:error:` y selectores Base64 (`initWithBase64EncodedData:options:`, `initWithBase64Encoding:`, `base64EncodedStringWithOptions:`, `base64EncodedDataWithOptions:`, `base64Encoding`).
   - `writeToURL:*` valida `[url isFileURL]`; para esquemas no-archivo devuelve `NO` con `NSCocoaErrorDomain` código `518` (`NSFileWriteUnsupportedSchemeError`).
4. **`fix(foundation): CFBundleGetAllBundles y NSBundle allBundles/allFrameworks` (`patches/02-...`, `patches/04-...`)**:
   - Implementado `CFBundleGetAllBundles` (evita `SIGSEGV` en `QtCore` `QLibraryInfo::path`).
   - `+[NSBundle allFrameworks]` devuelve solo bundles `.framework` (sin el bundle principal de la app); `+[NSBundle allBundles]` devuelve los no-framework (incluyendo el principal).
5. **`fix(darwin/mach,kqueue): MACH_SEND_TIMEOUT + kevent64 MACH_RCV_MSG` (`patches/01-darwin-gcd-mach-net-sandbox.patch`)**:
   - `crates/darwin/src/mach/wire.rs` respeta `MACH_SEND_TIMEOUT` usando `MSG_DONTWAIT` + espera acotada cuando el socket Unix dgram está lleno (`EAGAIN`).
   - `crates/darwin/src/kqueue.rs::kevent64` ahora implementa `MACH_RCV_MSG` real (recibe/drena el mensaje Mach en `(ext[0], ext[1])` con `mach_msg` no bloqueante y devuelve `mach_msg_return_t` en `event.fflags`). Antes `kevent64` descartaba `MACH_RCV_MSG`, haciendo que los 5 hilos `MessagePumpKqueue` de Chromium/TTNet girasen al 100% CPU sin drenar sus puertos y bloqueando `CrBrowserMain` (congelando la UI) en `sendmsg` (`unix_wait_for_peer`) al llenarse `max_dgram_qlen=10`.
6. **`fix(darwin/spawn): posix_spawnattr_set_csm_np` (`patches/01-...`, `patches/04-...`)**:
   - Implementado `posix_spawnattr_set_csm_np` (no-op aceptado en Linux, `EINVAL` si `attr == NULL`), resolviendo el `SIGSEGV at 0x0` desde `Chromium Embedded Framework+0x46093c6` al lanzar procesos helper de CEF.
7. **`fix(corefoundation,foundation): interning de NSString ASCII cortos` (`patches/02-cf-foundation-iokit-coretext.patch`)**:
   - Interning sin tope artificial como `__NSCFConstantString` inmortal para ASCII `<= 9` bytes únicamente cuando el receptor es exactamente la clase `NSString` en `+[NSString stringWithUTF8String:]` y `+[NSString stringWithCString:encoding:]` (8-bit).
   - `+[NSMutableString stringWithUTF8String:]` y `+[NSMutableString stringWithCString:encoding:]` siempre devuelven una copia mutable nueva (verificado con test en `crates/foundation/tests/string_init.rs`); subclases de `NSString` tampoco usan el interning.
   - Evita el double-free de Qt 6.2.2 `QRhiMetal` al liberar `[NSString stringWithUTF8String:"main0"]`.
8. **`fix(appkit): CALayer geometryFlipped/opaque/autoresizingMask y NSWindow keyViewSelectionDirection` (`patches/04-appkit-coregraphics-globals.patch`)**:
   - Añadidas propiedades reales `isGeometryFlipped`/`setGeometryFlipped:`/`contentsAreFlipped`, `isOpaque`/`setOpaque:`, `needsDisplayOnBoundsChange`/`setNeedsDisplayOnBoundsChange:`, `autoresizingMask`/`setAutoresizingMask:` en `CALayer` y `keyViewSelectionDirection` (más helpers de key-view loop marcados `STUB:`) en `NSWindow`, necesarios al desbloquear el renderizado de CEF.
9. **`fix(security): SecureTransport SSL* y SecPKCS12Import para libqsecuretransportbackend.dylib` (`patches/03-...`, `patches/04-...`)**:
   - Añadido `crates/security/src/ssl.rs` con `SSLContext` CF real (creación, cifrados soportados, setters/getters de estado/versión) y claves `kSecImport*`; handshake/read/write devuelven `errSSLClosedAbort` (`STUB:`) en lugar de crashear en `0x0` cuando Qt abre sockets TLS al crear un proyecto.
10. **`fix(appkit): NSWindow worksWhenModal y flags de panel` (`patches/04-appkit-coregraphics-globals.patch`)**:
    - Añadidos `worksWhenModal`, `setWorksWhenModal:`, `becomesKeyOnlyIfNeeded`, `setBecomesKeyOnlyIfNeeded:`, `isFloatingPanel`, `setFloatingPanel:` en `NSWindow`.
11. **`fix(foundation): NSLock y NSRecursiveLock reales` (`patches/02-cf-foundation-iokit-coretext.patch`)**:
    - Añadido `crates/foundation/src/lock.rs` con `pthread_mutex_t` real (normal para `NSLock`, `PTHREAD_MUTEX_RECURSIVE` para `NSRecursiveLock`), soportando `init`, `dealloc`, `lock`, `unlock`, `tryLock`, `lockBeforeDate:`, `name`, `setName:` (requerido por `libcccreator.dylib` al abrir el editor de vídeo).

---

## Lista "VERIFICAR EN LINUX REAL"

1. **`net.unix.max_dgram_qlen` y `CrBrowserMain` (`MACH_SEND_TIMEOUT`)**:
   - Comprobar valor de `sysctl net.unix.max_dgram_qlen` en CachyOS (por defecto 10 en Linux).
   - Verificar que `CrBrowserMain` no se queda colgado al abrir ventanas/webviews de CEF y comprobar si algún receptor Mach de Chromium/Rosemary deja mensajes sin drenar.
2. **Interning de `NSString` y `NSMutableString`**:
   - Ejecutar `cargo test -p corefoundation --lib` y `cargo test -p foundation --test string_init`.
   - Comprobar en sesión real prolongada que no hay double-frees al compilar shaders Metal ni fugas notables de memoria.
3. **Shaders Metal -> Vulkan (`QRhiMetal` / `EffectSDK`) con GPU real**:
   - En Colab solo tenemos Lavapipe (software). En CachyOS con GPU Vulkan real, comprobar que la ventana de inicio, los diálogos, el preview del editor y los efectos renderizan sin artefactos, sin errores de validación Vulkan ni cuelgues de GPU.
4. **`NSColorSpace` / `NSBundle` / `NSData` / `CFMachPortCreateRunLoopSource`**:
   - Verificar que colores/perfiles ICC se ven correctamente en pantalla real, que Qt carga plugins/traducciones desde los bundles correctos y que los archivos de configuración/proyectos de CapCut se escriben sin errores.
5. **Integridad de los parches**:
   - Confirmar que `./scripts/build-rosemary.sh` aplica los parches de aquella revisión limpios sobre el commit `411f2844cbd44754f727c2ebd22d7a168a506097` y compila sin errores.

---

## Resultados CachyOS

### 2026-11-25 — CachyOS x86_64, KDE Plasma Wayland 2560×1440 @ 1.25×, NVIDIA GeForce RTX 5060 Ti

Rosemary base `411f2844`, parches `patches/01`–`05` regenerados desde el árbol verificado
(`git apply --check` limpio sobre la base y los 76 ficheros resultantes idénticos byte a byte).

Lanzamiento:

```bash
ROSEMARY_DECORATIONS=server ROSEMARY_MENU=none ROSEMARY_KEEP_FAULT_HANDLER=1 \
  rosemary run --new-instance CapCut.app
```

Antes de cada lanzamiento hay que borrar los `*.locked` de 0 bytes de
`com.lveditor.draft/*/` (el launcher lo hace solo); un cierre anómalo de CapCut los deja.

**Verificado en vivo**

- Arranque, Home, editor y apertura de proyectos existentes (`1125 (7)`, `1125 (8)`).
- Escala fraccional Wayland 1.25× sin borde ni área 80 %×80 %.
- Ventana maximizada sin franja negra superior: CapCut añade un accesorio de barra de título
  de 36 pt y Rosemary encogía con él el content view `FullSizeContentView`; en AppKit el
  accesorio se superpone.
- Importar una foto `.jpg`: aparece en Medios, en la línea de tiempo con miniaturas y llega
  al reproductor; el panel de Efectos se abre.

**Arreglos de esta tanda** (en `04`/`05`)

- `CVMetalTextureCache*` y `CVOpenGLTextureCache*` reales (crash en `RendererMetalV2::convertBuffer`).
- `-[MTLDevice newRenderPipelineStateWithDescriptor:options:reflection:error:]`.
- `kCVImageBufferAlphaChannelModeKey` / `_StraightAlpha` (débiles; CapCut los desreferencia).
- QuickLook mínimo: `QLThumbnailImageCreate` (devuelve NULL, como sin generador) y `kQLThumbnailOptionIconModeKey`.
- MSL §5.2.1: argumentos sin `[[buffer/texture/sampler]]` toman el primer índice libre (los shaders de SPIRV-Cross de CapCut).
- `_Unwind_*` y ABI C++ de imágenes cargadas con `dlopen` ligados al libunwind/libc++abi del
  runtime: iban al `libgcc_s` del host, que no lee el compact unwind de macOS y abortaba
  (`libres_pool.dylib`, comprobado en el core dump).

**Segunda tanda (misma tarde)**

Verificado en vivo: menú contextual (clic derecho) transparente y en su sitio; importar varias fotos seguidas, miniaturas en el panel de Medios y en la
línea de tiempo (4 clips), botón Export activo. El panel de Medios ya no se atasca en bucle.

- MSL `isunordered`/`isordered`: los dos shaders de medios de CapCut fallaban al traducirse y
  CapCut los pedía sin parar (63 787 + 2 083 traducciones fallidas en ~20 s; caché 0 escrituras).
- SPIR-V: `Ordered`/`Unordered` se emiten con `OpIsNan` (`OpOrdered`/`OpUnordered` son de
  `Kernel` y el compilador de NVIDIA se caía con ellos); validado con `spirv-val --target-env vulkan1.3`.
- `-[MTLCommandEncoder device]`.
- Ventanas no opacas: `-isOpaque`/`-setOpaque:` reales, superficie con alfa y swapchain
  `PRE_MULTIPLIED` cuando el invitado la pide (el menú de Qt usa una ventana transparente a pantalla completa).
- Diagnóstico con variables de entorno: `ROSEMARY_TRACE_SIGNALS`, `ROSEMARY_TRACE_FRAMES`,
  `ROSEMARY_TRACE_SHADER_CACHE`.

**Pendiente**

- El editor sigue yendo con retardo al cargar contenido: CPU (~7 %) y GPU (~1 %) casi ociosas,
  así que es espera, no cómputo; por localizar.
- Menú contextual: **verificado en vivo** en Wayland (transparente, redondeado, en su sitio)
  con `4f3bc52`; aún va con retardo al abrirse.
- La pantalla de inicio no muestra miniaturas de proyectos ni el degradado del botón.
- El reproductor no reproduce y el editor va muy lento (~1 fps).
- Vídeo `.mp4`: implementadas las nueve funciones CoreVideo. La auditoría y el estado de las pruebas están al final.
- `frameworks::tests` informa 6 símbolos duplicados (`_CFNotificationCenter*` y
  `_NSURLTypeIdentifierKey`, `_kCGImageSourceTypeIdentifierHint`); dos tests de appkit fallan
  (`a_panel_keeps_its_becomes_key_only_if_needed`, `metal_scene_tracks_sibling_order_hiding_and_removal`).
- Al cerrar la ventana CapCut sigue vivo (comportamiento macOS) y sus helpers CEF quedan huérfanos al salir.

**Medidas de latencia (siguiente paso)**

Con `ROSEMARY_TRACE_WINDOW=1 ROSEMARY_METAL_STATS=1` (líneas con `[ms]` en un mismo reloj):

- Clic derecho → CapCut crea la ventana del menú: ~100–140 ms (lógica de CapCut; CPU casi
  ociosa, o sea espera). Ventana → primer frame: ~50 ms. Total ~150–190 ms.
- Render: ~125 fps, hueco máximo entre frames ~11 ms; la GPU no es el cuello de botella.
- Cada menú crea una ventana nueva a pantalla completa que se crea dos veces (al iniciarse y al
  saber en `orderFront` que es transparente). Crearla ya transparente ahorraría parte de los 50 ms.
- Menú opaco a veces: cuando además se rehace como ventana OpenGL (1 de cada ~10), su superficie
  GL no pide canal alfa.
- Estirar fotos en la línea de tiempo también va con retardo. Para ver en qué espera CapCut hace
  falta un muestreador nativo: `ROSEMARY_PROFILE` solo funciona en modo emulado, y gdb
  (lanzando con `PR_SET_PTRACER` para no necesitar `ptrace_scope=0`) la encuentra ociosa.

### Importación MP4 · 25 de noviembre de 2026

Codex implementó los tres conversores de color, la consulta de adjuntos, la apertura limpia, la consulta de formatos comprimidos, las dos consultas del display link y la creación de buffers que comparten un IOSurface. El retorno de CGRect incluye el adaptador para el puntero de resultado oculto de x86_64. El backend admite IOSurface empaquetados BGRA, RGBA y ARGB. Rechaza los formatos planares o comprimidos que carecen de respaldo.

**Verificación de código.** Compilación release con seis trabajos, 38 tests de CoreVideo y un test del retorno de CGRect a memoria del invitado. Se aplicaron los cinco parches en orden sobre la base fijada y el árbol de Git coincidió con el árbol compartido completo (90 rutas modificadas).

**Prueba de interfaz.** Home y un proyecto nuevo abrieron con Xvfb, Mesa EGL y Lavapipe en una copia del prefijo. El selector devolvió una selección vacía antes de entregar el H.264 de prueba (320 × 180, 24 fps, dos segundos). Clio abrió el proyecto existente `1125 (7)` en Wayland con la RTX 5060 Ti. Observó la previsualización en negro y bloqueos al reproducir. Esta sesión documenta un fallo del editor con recursos de imagen. Siguen pendientes la importación y la reproducción de un MP4 de prueba.

**Auditoría estática de libcccreator.dylib, slice x86_64 de CapCut 9.8.0.4799.** Las direcciones son virtuales de la imagen antes de ASLR. Se cruzaron las referencias a clases y selectores de `llvm-objdump --macho --arch=x86_64 --disassemble --symbolize-operands` con las implementaciones de métodos de `--objc-meta-data`.

| Código identificado | Receptor y mensajes relevantes |
| --- | --- |
| `IESMMFastVideoImport isSupportFastImport:mediaType:` (0x23800b0) | Crea el asset con `+[AVURLAsset assetWithURL:]`, consulta `tracksWithMediaType:` con `AVMediaTypeVideo` y lee `formatDescriptions` de la primera pista. |
| `VEVideoFileInfo ffmpegInit:` (0x24230a0) | Además del sondeo FFmpeg, la ruta construye un AVURLAsset con `assetWithURL:` y lee `metadata`. |
| `AVVideoReaderOCImpl openReader` (0x2fbdb60) e `initVideoReader:` (0x2fbca50) | Usa `+[AVAssetReader assetReaderWithAsset:error:]`, selecciona una pista de vídeo y crea la salida con `+[AVAssetReaderTrackOutput assetReaderTrackOutputWithTrack:outputSettings:]`. Configura `setAlwaysCopiesSampleData:` y añade la salida con `-[AVAssetReader addOutput:]`. |
| `VideoStickerReader createReaderAndDecoder:` (0x6117c0) y `getNextStickFrame` (0x611aba) | Ruta de stickers. Usa `URLAssetWithURL:options:`, `assetReaderWithAsset:error:`, `setTimeRange:`, `canAddOutput:`, `addOutput:` y `startReading`. Consulta `status` en el lector y `copyNextSampleBuffer` en la salida. |
| `VEAudioReader initAVAssetReader:audioConfig:` (0x2445290) | Ruta de audio. Usa `-[AVURLAsset initWithURL:options:]`, `assetReaderWithAsset:error:` y consulta las capacidades del asset. |

Estas referencias prueban que las rutas existen. La secuencia efectivamente ejecutada por el importador en Wayland queda pendiente de una traza. La inicialización de AVFoundation en este árbol registra AVCaptureDevice y AVAudioFormat. AVURLAsset, AVAssetReader y AVAssetReaderTrackOutput siguen pendientes.

### Reproductor en Wayland · prueba de Clio

- A las 20:11:14 Rosemary registró que otro hilo conservaba el contexto OpenGL y que `eglMakeCurrent` devolvió `EGL_BAD_ACCESS`. A las 20:11:36 falló la creación de framebuffers GLES y a las 20:11:44 falló la consulta de `GL_VERSION`.
- A las 20:11:46 el motor de efectos falló al crear el pipeline `Inplace-Vertical-Flip` y el renderizador Metal informó de una discrepancia entre geometría y atributos de vértice, mientras procesaba recursos del proyecto, entre ellos una imagen PNG. Los mismos mensajes de EGL y Metal aparecen en la sesión anterior de Claude. El registro actual no identifica la espera que bloquea la reproducción.
- Segunda prueba de Clio, con `ROSEMARY_GL_DEBUG`, `ROSEMARY_TRACE_FRAMES` y `ROSEMARY_TRACE_SHADER_CACHE`. El hilo 846 tenía el contexto primario de la ventana y el hilo 3 seleccionó un contexto compartido para la misma ventana. `eglMakeCurrent` devolvió `EGL_BAD_ACCESS`. La traza continuó con más cambios de contexto y alcanzó otra inicialización del motor de efectos. No se registraron estadísticas de presentaciones OpenGL ni errores del traductor de shaders en esta captura. La traza acota el fallo de activación del contexto, pero todavía falta localizar la espera al pulsar Play.
- **Arreglado (Antigravity, `patches/04-appkit-coregraphics-globals.patch`)**:
  - Causa confirmada en `crates/appkit/src/opengl.rs`: al agotarse `HANDOFF` (2 s), el hilo secundario creaba un contexto compartido (`sibling`) pero intentaba vincularlo a la **misma `EGLSurface`** que seguía retenida por el primer hilo (`EGL_BAD_ACCESS` por especificación EGL), dejando al segundo hilo sin contexto GL activo (`GL_VERSION` y FBOs GLES fallando).
  - Solución: `sdl3::video::egl::make_current_offscreen` (pbuffer EGL 1×1 por hilo con fallback `EGL_NO_SURFACE` + `clear_current`) y `BindResult::{Window, Offscreen}` en `bind_native_context`/`unbind_here`. Verificado en vivo en Wayland + RTX 5060 Ti (`/tmp/capcut-cachyos-egl-fix.log`): **`EGL_BAD_ACCESS` bajó a 0**, errores de `GL_VERSION` y FBOs GLES a **0**, 23 contiendas resueltas en superficie offscreen.
  - Además, se añadió `timed_out_owner` en `Shared` para que `claim_window` y `make_current_context` no repitan esperas de 2 s en cada llamada mientras el mismo `owner` siga reteniendo el contexto (antes 23 × 2 s = 46 s de pausa muerta).
  - **Lo que queda abierto en el reproductor y menús**:
    1. **Metal `MTLRenderPipelineReflection` / `MTLComputePipelineReflection` implementado (`patches/05-media-pipeline-metal-corevideo-runtime.patch`, Claude + Antigravity)**:
       - Añadidos `crates/metal/src/reflection.rs`, `crates/metal_translate/src/spirv/reflect.rs` y `crates/metal_translate/tests/source_reflection.rs` (reflexión previa a `widen` para `half`, matrices `float2x2..float4x4` en structs, `newRenderPipelineStateWithDescriptor:options:reflection:error:` y `newComputePipelineStateWith{Function,Descriptor}:options:reflection:error:`).
       - Verificado en vivo (`/tmp/capcut-metal-reflect.log`): **`AMGSwingBaseBlender.mm:300` (`Inplace-Vertical-Flip`) bajó a 0 errores** y todos los shaders traducen limpio.
       - **Arreglado (Antigravity, `patches/05-media-pipeline-metal-corevideo-runtime.patch`)**: `RendererMetalV2.mm:7083` (`geometry and vertex attributs don't match`) venía de `crates/metal_translate/src/air/metadata/mod.rs:459`: para `air.vertex_input`, `varying_link_name(&items)` encontraba `generated(__air_placeholder__)` y ocultaba `air.arg_name` (`aPosition`, etc.), devolviendo `user(generated(__air_placeholder__))` en `-[MTLFunction vertexAttributes]`. Ahora `air.vertex_input` lee siempre `argument_name(&items)` e `interface_reflection` corre antes de `widen` (tests `metal_translate` 100 % en verde).
    2. **TLS real en `SecureTransport` implementado (Antigravity, `patches/03-security-tls-codesign.patch` y `patches/05-...`)**:
       - `crates/security/src/ssl.rs` ahora usa `rustls` 0.23 (TLS 1.2/1.3 no bloqueante, `kSSLSessionOptionBreakOnServerAuth` -> `errSSLPeerAuthCompleted`, `SSLCopyPeerTrust` real para `SecTrustEvaluate`, ALPN, `SSLRead`/`SSLWrite`/`SSLClose`). 21/21 tests de `security` pasan, incluido handshake real con `editor-api.capcutapi.com:443`.
    3. **Arreglado (Antigravity, `patches/04-appkit-coregraphics-globals.patch`) — lentitud global del editor y menús opacos**:
       - Causa demostrada con `ROSEMARY_METAL_STATS=1` (`/tmp/capcut-vertex-fix-live.log`): en Home la ventana usaba swapchain Vulkan (`readback_micros=0`, `blit_micros=0`), pero al abrir el editor `EffectSDK`/`RenderEngineGLES30` creaba un `NSOpenGLContext` offscreen (`view == nil`) y `drawable_window()` en `crates/appkit/src/opengl.rs` caía a `key_window()`, reconstruyendo la ventana Metal 2560×1440 del editor (y 1 de cada ~10 menús contextuales) con `SDL_WINDOW_OPENGL`. Eso destruía su swapchain Vulkan y degradaba toda la UI Metal a **174 FPS de readback GPU→CPU + blit en el hilo principal** (`present_micros=257 ms/s`, `readback_micros=173 ms/s`, `blit_micros=156 ms/s`, `buffer_zero_micros=76 ms/s`).
       - Solución: `crates/appkit/src/view.rs::window_has_metal_layer` + `crates/appkit/src/opengl.rs::window_uses_metal` / `drawable_window` nunca secuestran ni reconstruyen como OpenGL una ventana que ya posee swapchain Vulkan o `CAMetalLayer`; los contextos GL sin vista (o sobre ventanas Vulkan activas) usan `holding_window` oculto 1×1. 19/19 tests de `appkit::opengl` en verde. Verificado en vivo en Wayland (`/tmp/capcut-swapchain-fix-live.log`): `readback_micros=0`, `blit_micros=0`, `gpu_micros ~0.4–3 ms/s`, popups en ~3 ms, editor fluido y lienzo 16:9 visible en el reproductor.
    4. **Arreglado (Antigravity, `patches/04` y `patches/05`) — crash al arrastrar imágenes y compartición `CVMetalTextureCache`**:
       - `-[NSWindow/NSView dragImage:at:offset:event:pasteboard:source:slideBack:]` implementado como no-op seguro (evita `SIGABRT` por selector no reconocido en `QNSWindow` al arrastrar).
       - `crates/corevideo/src/pixel_buffer/texture_cache.rs` + `pixel_buffer.rs` + `surface.rs`: `CVMetalTextureCacheCreateTextureFromImage` ahora solicita `ShaderRead | ShaderWrite | RenderTarget` (fallback `ShaderRead | RenderTarget`) y comparte un único `MTLTexture` por `(device, CVPixelBuffer/IOSurface, plane, pixel_format, size)` re-subiendo píxeles solo tras escrituras CPU (`cpu_generation` / `IOSurfaceGetSeed`) y limpiando al bajar el `use_count` del `IOSurface`. Todos los tests de `corevideo` y `appkit::opengl` en verde.
- Prioridad 1: revisar con Claude la inicialización del pipeline Metal (`Inplace-Vertical-Flip` / atributos de vértice) y la espera durante Play.
- Prioridad 2: repetir con un proyecto nuevo y un MP4 H.264, comprobando Medios, miniatura, primer fotograma y reproducción. Capturar la primera llamada que falle antes de ampliar AVFoundation.
- Prioridad 3: seguir con el muestreador nativo, la latencia del editor y la creación de la ventana del menú con alfa.

**Congelación al reproducir: resuelta (verificada en vivo)**

Era un interbloqueo: el hilo principal llamaba a `VERunSynchronouslyOnContextQueueWithParam`
(`dispatch_sync` a la cola de contexto del motor de vídeo) y Rosemary ejecutaba ese bloque en
un trabajador del pool; dentro, `libAGFX` volvía a pedir trabajo al hilo principal, que esperaba.
libdispatch ejecuta el bloque de `dispatch_sync` en el hilo que llama cuando le toca el turno.
Ahora `darwin::dispatch::queue` hace lo mismo (`Work::Handoff`) para toda cola salvo la principal,
conservando orden y exclusión; test `dispatch_sync_runs_on_the_callers_thread_in_order`.
Suite de `darwin`: 922/922.

**Reproductor en negro (siguiente)**

Con `ROSEMARY_TRACE_WINDOW=1` la escena Metal del editor contiene la capa de la vista previa
(`VEPreviewMetalView`, p. ej. `780x439@653,79`, opaca) encima de la de Qt, y se presentan ~42
drawables/s: la composición funciona y el contenido que dibuja AGFX sale negro. Sospechas:
texturas de entrada vacías (`CVMetalTextureCache` / pipeline Inplace-Vertical-Flip) o el pase final.
Falta una herramienta para volcar el contenido de un drawable/textura.

**CVDisplayLink: 60 hilos por segundo (resuelto)**

Qt para y arranca su `CVDisplayLink` en cada frame; Rosemary creaba un hilo nuevo en cada
`CVDisplayLinkStart` (~62 hilos/s). Ahora cada link tiene un hilo persistente que se duerme
en `Stop` y se despierta en `Start`. Medido en vivo: 312 hilos nuevos en 5 s -> 0. Test
`a_restarted_link_calls_back_on_the_same_thread`; corevideo 39/39.

**Red de CapCut: nada online funciona (abierto)**

Efectos, Animaciones, Exportar y el certificado C2PA fallan. Trazado en vivo con gdb durante
4 min pulsando Efectos: **cero** llamadas a `getaddrinfo` y **cero** `connect` TCP. Los hilos
TTNet (`libsscronet`) están ociosos, no bloqueados: la petición muere dentro de TTNet antes de
DNS. Descartado: `resolv.conf` (montado), `SCNetworkReachability` (responde reachable con ruta),
proxy (`CFNetworkCopySystemProxySettings` vacío). Los logs `alog` de CapCut están cifrados.

**`fabs` en MSL fuente (resuelto)**

AGFX fallaba 22 veces al crear pipelines (`AirSyntax: unsupported function fabs with 1 argument`).
`fabs` se traduce ahora como `FloatUnaryOp::Absolute` (GLSL.std.450 FAbs); test `source_msl_lowers_fabs`.
Tras el cambio: 0 fallos de pipeline. El reproductor **sigue en negro**, y al redimensionar la
ventana la capa del reproductor se dibuja fuera de su caja (blanco sobre timeline y Details)
aunque la escena reporta marcos correctos (`780x439@653,79`, `954x439@566,79`): hay tres capas
Metal del reproductor apiladas en el mismo sitio. Siguiente: volcar el contenido de cada drawable.

**Red: más hipótesis descartadas (26-nov, noche)**

- `SCDynamicStoreCopyProxies` devolviendo diccionario vacío en vez de NULL: probado en vivo, sin cambio
  (C2PA sigue con `biz response is null`, sin conexiones nuevas). Revertido.
- `NSRegularExpression` (único import sin enlazar de `libsscronet`): solo se usa en una función que
  valida un patrón (`libsscronet+0x411860`, devuelve `error == nil`); con la clase ausente devuelve
  "válido". No bloquea nada.
- Siguiente idea: poner breakpoints en las funciones de error de TTNet/Cronet (p. ej. donde se
  construye la respuesta de error de `URLRequest`) para ver qué código `net::ERR_*` se devuelve
  antes de la resolución DNS.

**Red de CapCut: RESUELTO (26-nov, tarde) — tres fallos encadenados**

1. **`kevent64` ignoraba `KEVENT_FLAG_IMMEDIATE`** (`darwin/src/kqueue.rs`). El `MessagePumpKqueue`
   de Chromium (hilo `TTNet-AN-IO` de `libsscronet`) sondea con ese flag y timeout nulo; Rosemary lo
   trataba como "esperar para siempre". El hilo de red se dormía tras el primer despertar y Chromium,
   creyendo que ya había un `ScheduleWork` pendiente, no volvía a enviar el mensaje Mach: ~40 peticiones
   `Cronet_UrlRequest_Start` aceptadas y ninguna procesada. Diagnóstico: gdb desde el arranque sobre
   `darwin::kqueue::kevent64` y `darwin::mach::msg::mach_msg` (puerto de despertar `EVFILT_MACHPORT`
   + `MACH_RCV_MSG`). Test `kevent64_with_immediate_flag_polls_despite_a_null_timeout`.
2. **`dlsym(dlopen(NULL), "dladdr")` devolvía NULL** (`rosemary/src/dyld.rs`). `libVECreator` resuelve
   `dladdr` así (puntero ofuscado con XOR 0x7e7) y lo llama al procesar respuestas: SIGSEGV en 0x0.
   `global_lookup` ahora consulta la tabla de funciones de dyld de Rosemary (`entries()`) antes del
   host (que rechaza `dladdr` por el `Dl_info` de glibc). Test `dlsym_finds_dyld_functions_in_the_global_scope`.
3. **`SecKeyCreateWithData` sin implementar**: nuevo `security/src/key.rs` con claves públicas RSA
   (PKCS#1 o SPKI), `SecKeyCreateEncryptedData` (`kSecKeyAlgorithmRSAEncryptionPKCS1`) y
   `SecKeyVerifySignature` (`kSecKeyAlgorithmRSASignatureDigestPKCS1v15SHA256`), con el crate `rsa` 0.10
   (solo operaciones de clave pública). Otros tipos/algoritmos se rechazan con `CFError`. Las constantes
   `kSecAttrKey*` y esos dos `kSecKeyAlgorithm*` pasan de `capcut_globals.rs` (valores de relleno) a
   `security` con los valores reales de Apple (`"type"`, `"42"`, `"kcls"`, `"0"`, `"algid:..."`). 3 tests.

Verificado en vivo (sin interacción): CapCut sigue vivo, 11 conexiones HTTPS a los servidores de
CapCut, `[TTC2PA_SDK] update local cert success`, aparece `User Data/TTNet` y se descargan
modelos en `Cache/effect/model`. Falta comprobar a mano Efectos/Animaciones/Exportar.

Nota: `kqueue::sources::tests::a_negative_interval_fires_once_at_once` es inestable al ejecutar solo
los tests de `kqueue` (también sin estos cambios); la suite completa de `darwin` pasa (923/923).

**⚠️ AVISO: dos cambios probados y REVERTIDOS (tumbaron KDE, 26-nov ~19:37)**

1. Quitar los stubs no-op de `CFNotificationCenter*` en `corefoundation` (para que ganara la
   implementación real de `foundation`): las notificaciones *distribuidas* empiezan a enviarse como
   mensajes Mach a los otros procesos del prefijo (helpers de CEF), que no los leen. Se acumulan y
   llenan el búfer del socket emisor único de `darwin::mach::wire` → todos los `mach_msg_send`
   bloquean (hilo principal en `sock_alloc_send_pskb`) y CapCut deja de responder.
2. "Arreglar" eso rotando el socket emisor cuando se llena: la cola deja de estar acotada y los
   descriptores que viajan con SCM_RIGHTS se acumulan hasta agotar el límite **por usuario** de
   descriptores en tránsito (`ETOOMANYREFS`). Eso rompe a *todos* los procesos del usuario:
   plasmashell/kwin murieron ("The Wayland connection experienced a fatal error: Demasiadas
   referencias") y hubo que reiniciar.

Conclusión: el bloqueo del emisor es lo que contiene el problema. Antes de activar las notificaciones
distribuidas reales hay que hacer que los procesos que las reciben drenen su puerto (o no enviarlas a
quien no escucha). Nunca hacer los envíos Mach no acotados.

Lo que sí funciona (con `282d878`): el usuario confirma que **los Efectos cargan**.

**Congelación al abrir + crash al aplicar efecto: resueltos (26-nov, noche)**

- **Mach: dos sockets emisores** (`darwin/src/mach/wire.rs`). Linux carga cada datagrama no leído
  (~2 KB aunque esté vacío) al socket que lo envió, y Rosemary usaba uno solo por proceso: con ~100
  puertos con un wakeup pendiente (normal en Chromium) se llenaba y *todo* `mach_msg_send` bloqueaba
  (hilo principal en `sock_alloc_send_pskb`). Ahora los mensajes **sin descriptores** usan un socket con
  `SO_SNDBUF` grande (4 MB, tope `net.core.wmem_max`) y los que **llevan derechos** siguen en el socket
  por defecto, que es lo que acota los descriptores en vuelo (límite por usuario; plasmashell tiene
  `RLIMIT_NOFILE` 1024, ver aviso de abajo). Test `unread_wakeups_on_many_ports_do_not_block_later_sends`
  (falla con el código anterior). Pendiente: `Chrome_IOThread` envía repetidamente un mensaje de 40 bytes
  con un derecho (`msgh_id` en la cabecera `...1314...73686a6d`) a un puerto que nunca lo lee; está acotado,
  pero si se llena vuelve a bloquear ese hilo.
- **`VTIsHardwareDecodeSupported`** (`videotoolbox`): faltaba y aplicar un efecto saltaba a 0x0.
  Devuelve false (aquí no hay decodificador hardware). Test incluido.
- **MSL: referencia `const` a un temporal** (`metal_translate`): `const thread float4& color` recibiendo
  una expresión fallaba con "color is passed something not in memory". Ahora se materializa en un
  local, como en C++; una referencia no-const sigue rechazándose. 2 tests.
- Herramienta: `ROSEMARY_PRESENT_READBACK=1 ROSEMARY_FRAME_OUT=<dir>` ahora nombra cada volcado con
  la capa y su tamaño y registra "% not black" por capa. ⚠️ Escribe ~6 MB por imagen: no usar `/tmp` (tmpfs).

Estado verificado por el usuario: Efectos cargan y se pueden aplicar sin crash; el reproductor ya dibuja
(el texto aparece) pero **los glifos salen como cajas blancas**; **Exportar no hace nada** (sin crash).

**Reproductor (texto + fotos) y redimensionado de ventana: RESUELTOS y verificados en vivo (26-nov, noche)**

1. **Glifos de texto como cajas blancas (`patches/05`)**:
   - `MTLDepthStencilDescriptor` devolvía copias desconectadas en `-frontFaceStencil` y `-backFaceStencil`, por lo que las mutaciones en sitio de AGFX sobre el descriptor de stencil se perdían y el stencil quedaba desactivado. Ahora se devuelven vistas vivas enlazadas al descriptor (y el getter de estado devuelve copia aislada). Tests `mutating_face_stencil_property_in_place_updates_descriptor_and_enables_stencil` y `reads_back_a_face_copy_and_isolates_source_mutations`.
   - También en `crates/metal`: texturas con swizzle (`A8Unorm` y `setSwizzle:`) que además llevan `STORAGE` ahora mantienen una vista de almacenamiento sin swizzle separada de la vista muestreada (`a8_unorm_texture_with_storage_usage_samples_as_alpha_and_supports_storage_write`, `swizzled_storage_texture_and_texture_view_preserve_sampling_swizzle`), y `R16G16B16A16_SFLOAT` se decodifica de forma segura en readback (`rgba16_float_readback_decodes_to_rgb24_and_counts_lit_pixels`).
   - Verificado en vivo por el usuario: texto SDF con borde/brillo y animaciones de texto se ven nítidos.
2. **Ventana descontrolada al redimensionar (`patches/04`)**:
   - `QNSView` sobrescribe `-layerContentsRedrawPolicy` devolviendo `2` (`NSViewLayerContentsRedrawDuringViewResize`), que Rosemary ignoraba (`ignored_integer`), dejando la `CALayer` contenedora sin redibujarse al cambiar el tamaño y sobresaliendo en blanco sobre la línea de tiempo y Details. Implementados `layerContentsRedrawPolicy`/`setLayerContentsRedrawPolicy:` sincronizando `CALayer.needsDisplayOnBoundsChange`, corregido el sombreado de `CALayer::setOpaque:`/`isOpaque`/`needsDisplayOnBoundsChange` en dispatch ObjC, y aplicado `child_clip` a `MetalSceneLayer.clip`.
   - Tests: `layer_contents_redraw_policy_during_view_resize_marks_backing_layer_for_display_on_resize`, `clips_to_bounds_round_trips_and_syncs_to_layer`, `metal_layer_opaque_and_bounds_display_flags_round_trip_through_objc_dispatch`, `metal_scene_places_an_oversized_drawable_by_gravity`.
   - Verificado en vivo por el usuario: el redimensionado funciona correctamente.
3. **Fotos en negro en el reproductor (`patches/04` y `patches/05`)**:
   - **Decodificación (`CoreImage` + `CoreVideo`)**: implementados `CIImage`, `CIContext`, `CIVector`, `CIFilter` y `CIColor` en `crates/appkit/src/coreimage.rs` (expuestos bajo el framework `CoreImage`) y los formatos empaquetados `RGBA`, `ARGB` y `RGhA` (`64RGBAHalf`) en `crates/corevideo/src/pixel_buffer.rs`. Tests `coreimage::tests` (3/3) y `packed_rgba_argb_and_rgba_half_buffers_allocate_and_expose_stride` (35/35 en `corevideo::pixel_buffer`).
   - **Reflexión AIR de `.metallib` (`crates/metal_translate`)**: causa raíz final por la que las fotos seguían saliendo negras aun decodificadas y subidas a `MTLTexture`. Los shaders precompilados de `VEMetalBinary_Mac.bundle` (`MVP_COMMON_vert.metallib`, `TEXTURE_MVP_RGBA_frag.metallib`, `Y_CB_CR_420_TO_RGBA_*`) no llevan nombres de parámetros en el IR LLVM (`arg0`, `arg1`...) sino en metadatos `air.arg_name` y `air.arg_type_name`, y envuelven `float4x4`/`float3x3` en `%"struct.metal::matrix"` e `int` sin signo. `spirv/reflect.rs` devolvía `arg0`/`arg1`, `MTLDataTypeStruct` y `UInt`, así que CapCut no encontraba `uMVPMatrix`, `uSamplerTexture`, `uPremultiplyAlpha` ni `uRgflip` y dibujaba con la textura dummy `1×1` negra y matriz cero. Corregido propagando `resource.name` y `air.arg_type_name` en `metal_translate` (`air_resource_metadata_names_and_buffer_types_override_unnamed_llvm_parameters`, 71/71 en `metal_translate`).
   - Verificado en vivo por el usuario: las fotos ya se renderizan en el reproductor detrás del texto.
4. **Crashes corregidos durante las pruebas (`patches/02`, `04`, `05`)**:
   - `NSFileHandle`: añadidos los selectores macOS 10.15 con `NSError**` (`seekToOffset:error:`, `seekToEndReturningOffset:error:`, `getOffset:error:`, `readDataUpToLength:error:`, `readDataToEndOfFileAndReturnError:`, `writeData:error:`, `truncateAtOffset:error:`, `synchronizeAndReturnError:`, `closeAndReturnError:`), evitando el crash al borrar un efecto. Test `error_returning_seek_read_write_and_truncate_methods_succeed_and_report_closed_handles` (5/5).
   - `NSLogv`: exportado en `frameworks.rs`, `shims/nslog.rs`, `shims/mod.rs` y `abi/table.rs` (evita SIGSEGV en 0x0 desde `libVECreator.dylib` al loguear con `va_list`). Test `nslogv_is_exported_and_renders_va_list_formats`.

**Pendientes reportados en vivo tras esta tanda**:
- Los botones superiores **Export** y **Pro** no registran clic.
- Al arrastrar elementos en la línea de tiempo queda un rectángulo gris flotante persistente sobre el escritorio.
- El menú de clic derecho vuelve a mostrar un fondo gris opaco a pantalla completa en lugar de transparente.
- Crash (`SIGSEGV`) al abrir/reproducir elementos de la **Library** de CapCut (fotos/vídeos online).


