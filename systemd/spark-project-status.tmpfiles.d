# spark-project-status.tmpfiles.d
#
# systemd-tmpfiles rules for spark-project-status.
# Place in /usr/lib/tmpfiles.d/ or /etc/tmpfiles.d/.
# Ensures RuntimeDirectory directories are cleaned on boot
# and have correct ownership.
#
# Format: type  path                  mode  uid  gid  age  argument
#
# d  = directory, create if missing, clean up old contents
#   path uses %i for the instance name
#   age = "d" means days; old directories beyond this are removed
#         (we use a generous age since reports are the output)
#
# Note: systemd already creates RuntimeDirectory on service start
# with correct permissions. This file handles cleanup of stale
# directories on boot and ensures they persist if the service
# is stopped.

d /run/spark-project-status 0755 spark-project-status spark-project-status - -
d /var/log/spark-project-status 0755 spark-project-status spark-project-status 30d -
