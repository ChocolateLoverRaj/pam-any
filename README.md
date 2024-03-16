# PAM Any
A PAM module that runs multiple other PAM modules in parallel, succeeding as long as one of them succeeds.

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
