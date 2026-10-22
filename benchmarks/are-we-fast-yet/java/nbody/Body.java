/* Adapted for ns-next from Are We Fast Yet
 * (https://github.com/smarr/are-we-fast-yet, commit
 * 74306fec151070fd07157cefeacf19e7e0bcdc89,
 * benchmarks/Java/src/nbody/Body.java).
 * Changed for ns-next: names are written out, PI as PI_VALUE (the piValue
 * of benchmarks/are-we-fast-yet/NBody.ns) and DAYS_PER_YER as
 * DAYS_PER_YEAR; the constructor's parameters, too long for two lines with
 * those names, are spread over three. Upstream's file carries no licence
 * notice; its comment follows as it is.
 */
/* The Computer Language Benchmarks Game
 * http://shootout.alioth.debian.org/
 *
 * Based on nbody.java and adapted basde on the SOM version.
 */
package nbody;

final class Body {
  private static final double PI_VALUE = 3.141592653589793;
  private static final double SOLAR_MASS = 4 * PI_VALUE * PI_VALUE;
  private static final double DAYS_PER_YEAR = 365.24;

  private double positionX;
  private double positionY;
  private double positionZ;
  private double velocityX;
  private double velocityY;
  private double velocityZ;
  private final double mass;

  public double getPositionX() { return positionX; }
  public double getPositionY() { return positionY; }
  public double getPositionZ() { return positionZ; }

  public double getVelocityX() { return velocityX; }
  public double getVelocityY() { return velocityY; }
  public double getVelocityZ() { return velocityZ; }

  public double getMass() { return mass; }

  public void setPositionX(final double positionX) { this.positionX = positionX; }
  public void setPositionY(final double positionY) { this.positionY = positionY; }
  public void setPositionZ(final double positionZ) { this.positionZ = positionZ; }

  public void setVelocityX(final double velocityX) { this.velocityX = velocityX; }
  public void setVelocityY(final double velocityY) { this.velocityY = velocityY; }
  public void setVelocityZ(final double velocityZ) { this.velocityZ = velocityZ; }

  void offsetMomentum(final double momentumX, final double momentumY, final double momentumZ) {
    velocityX = 0.0 - (momentumX / SOLAR_MASS);
    velocityY = 0.0 - (momentumY / SOLAR_MASS);
    velocityZ = 0.0 - (momentumZ / SOLAR_MASS);
  }

  Body(final double positionX, final double positionY, final double positionZ,
      final double velocityX, final double velocityY, final double velocityZ,
      final double mass) {
    this.positionX = positionX;
    this.positionY = positionY;
    this.positionZ = positionZ;
    this.velocityX = velocityX * DAYS_PER_YEAR;
    this.velocityY = velocityY * DAYS_PER_YEAR;
    this.velocityZ = velocityZ * DAYS_PER_YEAR;
    this.mass = mass * SOLAR_MASS;
  }

  static Body jupiter() {
    return new Body(
         4.84143144246472090e+00,
        -1.16032004402742839e+00,
        -1.03622044471123109e-01,
         1.66007664274403694e-03,
         7.69901118419740425e-03,
        -6.90460016972063023e-05,
         9.54791938424326609e-04);
  }

  static Body saturn() {
    return new Body(
         8.34336671824457987e+00,
         4.12479856412430479e+00,
        -4.03523417114321381e-01,
        -2.76742510726862411e-03,
         4.99852801234917238e-03,
         2.30417297573763929e-05,
         2.85885980666130812e-04);
  }

  static Body uranus() {
    return new Body(
         1.28943695621391310e+01,
        -1.51111514016986312e+01,
        -2.23307578892655734e-01,
         2.96460137564761618e-03,
         2.37847173959480950e-03,
        -2.96589568540237556e-05,
         4.36624404335156298e-05);
  }

  static Body neptune() {
    return new Body(
         1.53796971148509165e+01,
        -2.59193146099879641e+01,
         1.79258772950371181e-01,
         2.68067772490389322e-03,
         1.62824170038242295e-03,
        -9.51592254519715870e-05,
         5.15138902046611451e-05);
  }

  static Body sun() {
    return new Body(0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0);
  }
}
