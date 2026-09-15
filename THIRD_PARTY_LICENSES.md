# Third-Party Licenses

## Inspirateur/binary-greedy-meshing

This project includes portions of code taken and modified from binary-greedy-meshing.

Source: <https://github.com/Inspirateur/binary-greedy-meshing>

License: MIT

Copyright (c) 2024 Teo Orthlieb

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

## ash-rs/ash

This project includes portions of code taken from the Ash project.

Source: <https://github.com/ash-rs/ash>

License: MIT 

Copyright (c) 2016 ASH

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.

## Traverse-Research/gpu-allocator

This project includes `pomme-gpu-allocator`, a modified port of gpu-allocator
adapted for use with the pyronyx Vulkan bindings.

Source: <https://github.com/Traverse-Research/gpu-allocator>

License: MIT 

Copyright (c) 2021 Traverse Research B.V.
Copyright (c) 2026 Arsenii Nepeyvoda (arse09)

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

## Steel-Foundation/SteelMC

`pomme-singleplayer` links SteelMC, vendored as the `third_party/SteelMC`
submodule, to run singleplayer worlds inside the client. Because SteelMC is
AGPL-3.0-or-later, `pomme-singleplayer` is too; GPLv3 section 13 permits the
combination with the GPL-3.0-or-later client. Builds with
`--no-default-features` do not include it.

Source: <https://github.com/Steel-Foundation/SteelMC>

License: AGPL-3.0-or-later. Full text at
[third_party/SteelMC/LICENSE](./third_party/SteelMC/LICENSE).

## kcat/openal-soft

Release archives bundle, and `just client-*` stages next to dev binaries, the
unmodified OpenAL Soft native library shipped with Minecraft 26.2's LWJGL
natives. The exact version, jar member and checksum are recorded in
`pomme-client/third-party/openal-natives.txt`.

Source: <https://github.com/kcat/openal-soft>

License: GNU Library General Public License version 2. Full text at
[pomme-client/third-party/OpenAL-Soft-LICENSE.txt](./pomme-client/third-party/OpenAL-Soft-LICENSE.txt),
with the bundling notice in
[pomme-client/third-party/OpenAL-Soft-NOTICE.txt](./pomme-client/third-party/OpenAL-Soft-NOTICE.txt).

## azalea-rs/azalea

`pomme-block` is a data-free shim patched in place of azalea-block, keeping its
public types so the rest of azalea compiles while block semantics come from
Pomme's own per-version tables. Azalea itself remains a dependency for the
typed protocol layer.

Source: <https://github.com/azalea-rs/azalea>

License: MIT

Copyright (c) 2022 mat

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
