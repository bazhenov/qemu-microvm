## Building

1. Downloading Linux kernel
   ```
   $ make linux
   ```
2. Configuring the kernel
   ```
   $ cd linux
   $ make olddefconfig
   ```
3. Compile the kernel
   ```
   $ cd linux
   $ make -j$(nproc) // or make -j8 for using 8 cores
   ```
