terraform {
  required_version = ">= 1.5"
}

# The region to deploy to.
variable "region" {
  type    = string
  default = "eu-north-1"
}

resource "aws_instance" "web" {
  ami           = var.ami
  instance_type = "t3.micro"
  tags = {
    Name = "web-${var.region}"
  }
  count = length(var.zones) > 0 ? 1 : 0
}
