# PyInstaller spec for the current FOLO studio (RB-01 step 10): a one-dir
# build that CI smoke-tests with --help on Windows and Linux.
#
#   uv sync --group packaging
#   uv run pyinstaller --noconfirm packaging/folo-studio.spec
#   dist/folo-studio/folo-studio --help
#
# The audio capture (WASAPI on Windows, libpulse on Linux) declares its C
# types for cffi when it is imported, so cffi's backend and parser come
# along, and on Windows the winrt packages that read its media sessions.
# Linux uses the system's libpulse, which isn't bundled. The package lives in
# source/, so that's on the search path.

import sys
from pathlib import Path

from PyInstaller.utils.hooks import collect_submodules

ROOT = Path(SPECPATH).parent  # noqa: F821 - defined by PyInstaller

datas = [(str(ROOT / 'web'), 'web')]
hiddenimports = []
if sys.platform in ('win32', 'linux'):
    hiddenimports += ['_cffi_backend', *collect_submodules('pycparser')]
if sys.platform == 'win32':
    hiddenimports += collect_submodules('winrt')

a = Analysis(  # noqa: F821
    [str(ROOT / 'studio.py')],
    pathex=[str(ROOT), str(ROOT / 'source')],
    datas=datas,
    hiddenimports=hiddenimports,
    excludes=['tkinter'],
    noarchive=False,
)
pyz = PYZ(a.pure)  # noqa: F821
exe = EXE(  # noqa: F821
    pyz,
    a.scripts,
    [],
    exclude_binaries=True,
    name='folo-studio',
    console=True,
)
coll = COLLECT(exe, a.binaries, a.datas, name='folo-studio')  # noqa: F821
