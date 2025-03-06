{ disk, ... }: {
  disko.devices = {
    disk = {
      main = {
        device = disk;
        type = "disk";
        content = {
          type = "gpt";
          partitions = {
            ESP = {
              type = "EF00";
              size = "1G";
              content = {
                type = "filesystem";
                format = "vfat";
                mountpoint = "/boot";
              };
            };
            luks = {
              name = "luks";  # Set the partition label for easier identification
              size = "100%";
              content = {
                type = "luks";
                name = "crypted";
                # Settings for a more secure encryption setup
                settings = {
                  allowDiscards = true;      # Allow TRIM commands to pass through to the device (reduces lifespan but improves performance)
                };
                passwordFile = "/tmp/luks-password";  # Path to a file containing the password
                content = {
                  type = "filesystem";
                  format = "ext4";
                  mountpoint = "/";
                };
              };
            };
          };
        };
      };
    };
  };
}