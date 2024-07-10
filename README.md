# PAM Any
A PAM module that runs multiple other PAM modules in parallel, succeeding as long as one of them succeeds.

## Installation
Install [Rust](https://www.rust-lang.org/learn/get-started)

Build (`cargo build --release`)

Copy the PAM module to the location where PAM modules belong
```bash
sudo cp target/release/libpam_any.so /lib64/security
```
Depending on the distro, the folder might be `/lib` or `/lib64`. On Fedora it's `/lib64`.

Create / edit a file in `/etc/pam.d`. For example, `/etc/pam.d/sudo` for sudo authentication. Here is an example file:
```
auth sufficient libpam_any.so { "mode": "One", "modules": { "login": "Password", "pam-random": "Random Chance" } }
```
The text after `libpam_any.so` is a JSON object:
`mode`: Can be either `"One"` or `"All"`. "One" means that you can authenticate with any of the specified methods. For example, you can *either* type your password or use your fingerprint. "All" means that you must authenticate all of the specified modules, but in any order.
`modules`: Is an object where the key is a file that exists in `/etc/pam.d` and the value is the display name of the service. In the example above, `pam-any` will internally start PAM authentication based on the `/etc/pam.d/login` file (Text password), and `/etc/pam.d/pam-random` file [(A test module that randomly succeeds / fails)](https://github.com/ChocolateLoverRaj/pam-random). As soon as one of the two modules authenticates successfully, the `pam-any` module will authenticate successfully. If all sub-modules fail (wrong password), then `pam-any` will fail.

## Development
I created a VM to test stuff without messing up the distro I code in.
- Create a Fedora VM (can probably be any distro)
- Create a user named `test`
- Enable SSH server
- Enable root password
- Enable root SSH
- Setup password-less SSH login
- Setup [`pam-random`](https://github.com/ChocolateLoverRaj/pam-random) as a 2nd test module
- Update the `IP` variable in `test.sh`
- Run `bash ./test.sh`
- Inside the VM install `pamtester`
- Inside the VM run `pamtester pam-any test authenticate`
